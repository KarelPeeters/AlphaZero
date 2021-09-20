use std::time::Instant;

use board_game::games::ataxx::AtaxxBoard;
use board_game::util::bot_game;

use alpha_zero::mapping::ataxx::AtaxxStdMapper;
use alpha_zero::network::cpu::CPUNetwork;
use alpha_zero::network::cudnn::CudnnNetwork;
use alpha_zero::network::Network;
use alpha_zero::old_zero::{Tree, zero_build_tree, ZeroBot, ZeroSettings};
use alpha_zero::util::PanicRng;
use cuda_sys::wrapper::handle::Device;

fn main() {
    // let torch_path = "../data/derp/good_test_loop/gen_40/model_1_epochs.pt";
    let onnx_path = "../data/derp/good_test_loop/gen_40/model_1_epochs.onnx";
    // let onnx_v7_path = "../data/derp/good_test_loop/gen_40/model_1_epochs_v7.onnx";

    // let mut torch_network = AtaxxTorchNetwork::load(torch_path, tch::Device::Cuda(0));
    let mut cnn_network = CudnnNetwork::load(AtaxxStdMapper, onnx_path, 1, Device::new(0));
    let mut cpu_network = CPUNetwork::load(AtaxxStdMapper, onnx_path, 1);
    // let mut onnx_network = AtaxxOnnxNetwork::load(onnx_v7_path);

    println!("Root board eval");
    let board = AtaxxBoard::default();
    println!("{}", board);

    // println!("{:?}", torch_network.evaluate(&board));
    println!("{:?}", cpu_network.evaluate(&board));
    println!("{:?}", cnn_network.evaluate(&board));
    // println!("{:?}", onnx_network.evaluate(&board));

    println!("Tree");
    fn tree(network: &mut impl Network<AtaxxBoard>) -> Tree<AtaxxBoard> {
        let board = AtaxxBoard::default();
        let start = Instant::now();
        let tree = zero_build_tree(
            &board,
            100, ZeroSettings::new(2.0, false),
            network,
            &mut PanicRng,
            || false,
        );
        println!("Took {}s", (Instant::now() - start).as_secs_f32());
        tree
    }

    // println!("Torch:");
    // println!("{}", tree(&mut torch_network).display(4));

    println!("CPU:");
    println!("{}", tree(&mut cpu_network).display(4));

    println!("CNN:");
    println!("{}", tree(&mut cnn_network).display(4));

    // println!("ONNX:");
    // println!("{}", tree(&mut onnx_network).display(4));

    println!("bot_game");
    let settings = ZeroSettings::new(2.0, false);
    println!("{:#?}", bot_game::run(
        || AtaxxBoard::default(),
        || {
            let network = CPUNetwork::load(AtaxxStdMapper, onnx_path, 1);
            ZeroBot::new(100, settings, network, PanicRng)
        },
        || {
            let network = CudnnNetwork::load(AtaxxStdMapper, onnx_path, 1, Device::new(0));
            ZeroBot::new(100, settings, network, PanicRng)
        },
        1, true, Some(1),
    ));
}