use board_game::board::Board;
use board_game::wdl::{Flip, OutcomeWDL};
use decorum::N32;
use internal_iterator::{InternalIterator, IteratorExt};
use itertools::Itertools;

use crate::mapping::BoardMapper;
use cuda_nn_eval::quant::QuantizedStorage;
use kz_util::top_k_indices_sorted;

use crate::muzero::node::{MuNode, MuNodeInner};
use crate::muzero::tree::MuTree;
use crate::muzero::MuZeroEvaluation;
use crate::zero::node::{UctWeights, ZeroValues};
use crate::zero::range::IdxRange;
use crate::zero::step::FpuMode;

#[derive(Debug)]
pub enum MuZeroRequest<B> {
    Root {
        node: usize,
        board: B,
    },
    Expand {
        node: usize,
        state: QuantizedStorage,
        move_index: usize,
    },
}

#[derive(Debug)]
pub struct MuZeroResponse<'a> {
    pub node: usize,
    pub state: QuantizedStorage,
    pub eval: MuZeroEvaluation<'a>,
}

pub fn muzero_step_gather<B: Board>(
    tree: &MuTree<B>,
    weights: UctWeights,
    use_value: bool,
    fpu_mode: FpuMode,
) -> Option<MuZeroRequest<B>> {
    if tree[0].inner.is_none() {
        return Some(MuZeroRequest::Root {
            node: 0,
            board: tree.root_board().clone(),
        });
    }

    let mut curr_node = 0;
    let mut fpu = ZeroValues::from_outcome(OutcomeWDL::Draw, 0.0);

    let mut last_move_index = None;
    let mut last_state: Option<QuantizedStorage> = None;

    loop {
        let inner = if let Some(inner) = &tree[curr_node].inner {
            inner
        } else {
            return Some(MuZeroRequest::Expand {
                node: curr_node,
                state: last_state.unwrap(),
                move_index: last_move_index.unwrap(),
            });
        };

        // update fpu
        if tree[curr_node].visits > 0 {
            fpu = tree[curr_node].values();
        }
        //TODO should this be flip or parent? or maybe child?
        fpu = fpu.flip();

        // continue selecting, pick the best child
        let parent_total_visits = tree[curr_node].visits;

        let selected_index = inner
            .children
            .iter()
            .position_max_by_key(|&child| {
                let x = tree[child]
                    .uct(parent_total_visits, fpu_mode.select(fpu), use_value)
                    .total(weights);
                N32::from_inner(x)
            })
            .expect("Children cannot be be empty");

        let selected = inner.children.get(selected_index);

        curr_node = selected;

        last_move_index = Some(selected_index);
        last_state = Some(inner.state.clone());
    }
}

/// The second half of a step. Applies a network evaluation to the given node,
/// by setting the child policies and propagating the wdl back to the root.
/// Along the way `virtual_visits` is decremented and `visits` is incremented.
pub fn muzero_step_apply<B: Board, M: BoardMapper<B>>(
    tree: &mut MuTree<B>,
    top_moves: usize,
    response: MuZeroResponse,
    mapper: M,
) {
    let MuZeroResponse {
        node,
        state,
        eval: MuZeroEvaluation { values, policy },
    } = response;

    let lengths = [tree.mapper_policy_len, mapper.policy_len(), policy.len()];
    assert!(
        lengths.iter().all(|&x| x == policy.len()),
        "Mismatching policy lengths: {:?}",
        lengths
    );

    // create children
    let children = if node == 0 {
        // only keep available moves for root node
        let board = &tree.root_board;
        let indices = board.available_moves().map(|mv| mapper.move_to_index(&board, mv));
        create_child_nodes(&mut tree.nodes, node, indices, &policy)
    } else {
        // keep all moves deeper in the tree
        // TODO use the fact that moves are sorted by policy to optimize UCT calculations later on
        // TODO this doesn't work for the pass move, maybe it's finally time to retire it
        let mapped = policy.iter().copied().map(N32::from_inner);
        let indices = top_k_indices_sorted(mapped, top_moves).into_iter().map(Some);
        create_child_nodes(&mut tree.nodes, node, indices.into_internal(), &policy)
    };

    // set node inner
    let inner = MuNodeInner {
        state,
        net_values: values,
        children,
    };
    tree[node].inner = Some(inner);

    // propagate values
    tree_propagate_values(tree, node, values);
}

fn create_child_nodes(
    nodes: &mut Vec<MuNode>,
    parent_node: usize,
    indices: impl InternalIterator<Item = Option<usize>>,
    policy: &[f32],
) -> IdxRange {
    let start = nodes.len();
    let mut total_p = 0.0;

    indices.for_each(|index| {
        let p = index.map_or(1.0, |index| policy[index]);
        total_p += p;
        nodes.push(MuNode::new(Some(parent_node), index, p))
    });

    let end = nodes.len();

    // re-normalize policy
    for node in start..end {
        nodes[node].net_policy /= total_p;
    }

    IdxRange::new(start, end)
}

/// Propagate the given `values` up to the root.
fn tree_propagate_values<B: Board>(tree: &mut MuTree<B>, node: usize, mut values: ZeroValues) {
    values = values.flip();
    let mut curr_index = node;

    loop {
        let curr_node = &mut tree[curr_index];

        curr_node.visits += 1;
        curr_node.sum_values += values;

        curr_index = match curr_node.parent {
            Some(parent) => parent,
            None => break,
        };

        values = values.parent();
    }
}
