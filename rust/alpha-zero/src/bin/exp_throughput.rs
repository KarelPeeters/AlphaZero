use std::time::Instant;

use itertools::Itertools;
use rand::thread_rng;

use alpha_zero::games::ataxx_cnn_network::AtaxxCNNNetwork;
use alpha_zero::network::Network;
use board_game::games::ataxx::AtaxxBoard;
use board_game::util::board_gen::random_board_with_moves;
use cuda_sys::wrapper::handle::Device;

fn main() {
    let path = "../data/ataxx/test_loop/training/gen_264/model_1_epochs.onnx";
    let mut rng = thread_rng();

    let iterations = 10_000;

    for batch_size in [256] {
        let mut network = AtaxxCNNNetwork::load(path, batch_size, Device::new(0));
        let boards = (0..batch_size)
            .map(|_| random_board_with_moves(&AtaxxBoard::default(), 2, &mut rng))
            .collect_vec();
        for _ in 0..iterations {
            network.evaluate_batch(&boards);
        }

        let start = Instant::now();
        for _ in 0..iterations {
            network.evaluate_batch(&boards);
        }
        let delta = (Instant::now() - start).as_secs_f32();

        let games = iterations * batch_size;
        let throughput = games as f32 / delta;

        println!("Batch size {}:\t{} evals/s", batch_size, throughput as usize);
    }
}