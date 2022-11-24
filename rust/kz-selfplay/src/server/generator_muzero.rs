use std::borrow::Cow;
use std::cmp::min;

use board_game::board::AltBoard;
use flume::{Receiver, TryRecvError};
use internal_iterator::InternalIterator;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rand_distr::Dirichlet;

use cuda_nn_eval::quant::QuantizedStorage;
use cuda_sys::wrapper::handle::Device;
use cuda_sys::wrapper::mem::pool::DevicePool;
use kz_core::mapping::BoardMapper;
use kz_core::muzero::step::{
    muzero_step_apply, muzero_step_gather, MuZeroExpandRequest, MuZeroRequest, MuZeroResponse, MuZeroRootRequest,
};
use kz_core::muzero::tree::MuTree;
use kz_core::muzero::MuZeroEvaluation;
use kz_core::network::common::{softmax_in_place, unsoftmax_in_place};
use kz_core::network::muzero::{ExpandArgs, ExpandClient, RootArgs, RootClient};
use kz_core::network::ZeroEvaluation;
use kz_core::zero::step::{FpuMode, QMode};

use crate::move_selector::MoveSelector;
use crate::server::protocol::{GeneratorUpdate, Settings};
use crate::server::server::UpdateSender;
use crate::simulation::{Position, Simulation};

// TODO we're reusing quantized states from one network as inputs to a potentially updated network
//   hopefully that doesn't cause too many issues
pub async fn generator_muzero_main<B: AltBoard, M: BoardMapper<B>>(
    generator_id: usize,
    device: Device,
    start_pos: impl Fn(&mut StdRng) -> B,
    mapper: M,
    saved_state_channels: usize,
    settings_receiver: Receiver<Settings>,
    root_client: RootClient<B>,
    expand_client: ExpandClient,
    update_sender: UpdateSender<B>,
) {
    // wait for initial settings
    let mut settings = settings_receiver.recv_async().await.unwrap();
    let mut pool: Option<DevicePool> = None;

    let mut rng = StdRng::from_entropy();
    let state_size = saved_state_channels * mapper.state_board_size() * mapper.state_board_size();

    loop {
        // possibly get new settings
        match settings_receiver.try_recv() {
            Ok(new_settings) => settings = new_settings,
            Err(TryRecvError::Empty) => (),
            Err(TryRecvError::Disconnected) => break,
        }

        // possibly (re)allocate pool
        let pool_size = settings.full_iterations as usize * state_size;
        pool = pool.filter(|p| {
            assert_eq!(p.buffer().shared_count(), 1);
            p.total_size_bytes() == pool_size
        });
        let pool = pool.get_or_insert_with(|| DevicePool::new(device, pool_size));

        // send an update
        update_sender
            .send(GeneratorUpdate::StartedSimulation { generator_id })
            .unwrap();

        // actually generate a full game
        let simulation = generate_simulation(
            generator_id,
            &settings,
            &update_sender,
            &root_client,
            &expand_client,
            start_pos(&mut rng),
            mapper,
            state_size,
            pool,
            &mut rng,
        )
        .await;

        // send finished simulation
        update_sender
            .send(GeneratorUpdate::FinishedSimulation {
                generator_id,
                simulation,
            })
            .unwrap();
    }
}

async fn generate_simulation<B: AltBoard, M: BoardMapper<B>>(
    generator_id: usize,
    settings: &Settings,
    update_sender: &UpdateSender<B>,
    root_client: &RootClient<B>,
    expand_client: &ExpandClient,
    start: B,
    mapper: M,
    state_size: usize,
    pool: &mut DevicePool,
    rng: &mut impl Rng,
) -> Simulation<B> {
    let mut positions = vec![];

    let max_moves = settings.max_game_length.unwrap_or(u64::MAX) as u32;
    let use_value = match settings.q_mode.0 {
        QMode::Value => true,
        QMode::WDL { draw_score } => {
            assert_eq!(draw_score, 0.0, "MuZero does not yet support a nonzero draw score");
            false
        }
    };

    let mut curr_board = start;

    while !curr_board.is_done() {
        // check if we should consider this board a draw
        let draw_depth = max_moves - positions.len() as u32;
        if draw_depth == 0 {
            break;
        }

        // determinate search settings
        let is_full_search = rng.gen_bool(settings.full_search_prob);
        let target_visits = if is_full_search {
            settings.full_iterations
        } else {
            settings.part_iterations
        };

        // run tree search
        let mut tree = MuTree::new(curr_board.clone(), draw_depth, mapper);

        let root_max_moves = B::all_possible_moves().count();
        let inner_max_moves = min(settings.top_moves, mapper.policy_len());
        let max_nodes = 1 + root_max_moves + target_visits as usize * inner_max_moves;
        tree.reserve(max_nodes);

        let mut root_net_eval = None;

        while tree.root_visits() < target_visits {
            let request = muzero_step_gather(&mut tree, settings.weights.to_uct(), use_value, FpuMode::Relative(0.0));

            if let Some(request) = request {
                let output_state = QuantizedStorage::new(pool.alloc(state_size), state_size);

                let response = match request {
                    MuZeroRequest::Root(MuZeroRootRequest { node, board }) => {
                        let root_args = RootArgs {
                            board: board.clone(),
                            output_state: output_state.clone(),
                        };

                        let mut eval = root_client.map_async_single(root_args).await;

                        root_net_eval = Some(extract_zero_eval(mapper, &board, &eval));

                        add_dirichlet_noise(eval.policy_logits.to_mut(), settings, &board, mapper, rng);

                        MuZeroResponse {
                            node,
                            eval,
                            state: output_state,
                        }
                    }
                    MuZeroRequest::Expand(MuZeroExpandRequest {
                        node,
                        state,
                        move_index,
                    }) => {
                        let expand_args = ExpandArgs {
                            state,
                            move_index,
                            output_state: output_state.clone(),
                        };
                        let eval = expand_client.map_async_single(expand_args).await;

                        MuZeroResponse {
                            node,
                            eval,
                            state: output_state,
                        }
                    }
                };

                muzero_step_apply(&mut tree, settings.top_moves, response);
            }
        }

        // extract stats
        let net_evaluation = root_net_eval.unwrap();
        let zero_evaluation = tree.eval();

        //pick a move to play
        let move_selector = MoveSelector::new(settings.temperature, settings.zero_temp_move_count);
        let picked_index = move_selector.select(positions.len() as u32, zero_evaluation.policy.as_ref(), rng);
        let picked_child = tree[0].inner.as_ref().unwrap().children.get(picked_index);
        let picked_move_index = tree[picked_child].last_move_index.unwrap();
        let picked_move = mapper.index_to_move(tree.root_board(), picked_move_index).unwrap();

        // record position
        let position = Position {
            board: curr_board.clone(),
            is_full_search,
            played_mv: picked_move,
            zero_visits: tree.root_visits(),
            zero_evaluation,
            net_evaluation,
        };
        positions.push(position);

        // actually play the move
        curr_board.play(picked_move);

        // send update
        update_sender
            .send(GeneratorUpdate::FinishedMove {
                generator_id,
                curr_game_length: positions.len(),
            })
            .unwrap();

        // at this point we don't need the tree nor the underlying pool allocations any more
        drop(tree);
        unsafe {
            // we can't just clear here, since the there might still be leftover references to the states in the executors
            pool.clear_unsafe();
        }
    }

    Simulation {
        positions,
        final_board: curr_board,
    }
}

fn add_dirichlet_noise<B: AltBoard, M: BoardMapper<B>>(
    policy_logits: &mut [f32],
    settings: &Settings,
    board: &B,
    mapper: M,
    rng: &mut impl Rng,
) {
    // TODO consider using KataGo's shaped dirichlet noise, it's even more relevant for muzero
    //   is that true? we're still just adding noise to the available moves!

    let alpha = settings.dirichlet_alpha;
    let eps = settings.dirichlet_eps;

    // we're working on the logits here, so first take the softmax and then later un-softmax it
    let policy = policy_logits;
    softmax_in_place(policy);

    let mv_count = board.available_moves().count();
    if mv_count > 1 {
        let distr = Dirichlet::new_with_size(alpha, mv_count).unwrap();
        let noise = rng.sample(distr);

        board.available_moves().enumerate().for_each(|(i, mv)| {
            let mi = mapper.move_to_index(board, mv);
            policy[mi] = policy[mi] * (1.0 - eps) + noise[i] * eps;
        });
    }

    unsoftmax_in_place(policy, 0.0);
}

fn extract_zero_eval<B: AltBoard, M: BoardMapper<B>>(
    mapper: M,
    board: &B,
    eval: &MuZeroEvaluation,
) -> ZeroEvaluation<'static> {
    //TODO maybe also collect valid mass here?

    let mut policy: Vec<f32> = board
        .available_moves()
        .map(|mv| {
            let index = mapper.move_to_index(board, mv);
            eval.policy_logits[index]
        })
        .collect();

    softmax_in_place(&mut policy);

    ZeroEvaluation {
        values: eval.values,
        policy: Cow::Owned(policy),
    }
}
