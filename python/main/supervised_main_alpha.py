import os
import re
from typing import Optional

import torch

from lib.data.file import DataFile
from lib.data.group import DataGroup
from lib.data.sampler import PositionSampler
from lib.games import Game
from lib.logger import Logger
from lib.model.model_perceiver import ChessPerceiverModel
from lib.plotter import LogPlotter, run_with_plotter
from lib.supervised import supervised_loop
from lib.train import TrainSettings, ScalarTarget
from lib.util import DEVICE, print_param_count


def find_last_finished_batch(path: str) -> Optional[int]:
    if not os.path.exists(path):
        return None

    last_finished = -1
    for file in os.listdir(path):
        m = re.match(r"network_(\d+).onnx", file)
        if m:
            last_finished = max(last_finished, int(m.group(1)))

    return last_finished if last_finished >= 0 else None


def main(plotter: LogPlotter):
    output_folder = "../../data/supervised/perceiver_4"

    paths = [
        fr"C:\Documents\Programming\STTT\kZero\data\loop\chess\16x128_pst\selfplay\games_{i}.bin"
        for i in range(2400, 3400)
    ]

    limit_file_count: Optional[int] = None

    game = Game.find("chess")
    os.makedirs(output_folder, exist_ok=True)
    allow_resume = True

    batch_size = 64
    train_random_symmetries = False

    test_steps = 16
    save_steps = 128
    test_fraction = 0.05

    settings = TrainSettings(
        game=game,
        value_weight=0.1,
        wdl_weight=1.0,
        policy_weight=1.0,
        sim_weight=0.0,
        moves_left_delta=20,
        moves_left_weight=0.0,
        clip_norm=5.0,
        scalar_target=ScalarTarget.Final,
        train_in_eval_mode=False,
        mask_policy=True,
    )
    include_final: bool = False

    def initial_network():
        # return DenseNetwork(game=game, depth=16, size=2048, res=True, )
        # return ChessPerceiverModel(game, 128, 4, 512, 2, 12, False)
        return ChessPerceiverModel(game, 128, 4, 512, 2, 4, False)

    files = sorted((DataFile.open(game, p) for p in paths), key=lambda f: f.info.timestamp)
    if limit_file_count is not None:
        files = files[-min(limit_file_count, len(files)):]

    train_group = DataGroup.from_files(game, files, 0, 1 - test_fraction)
    test_group = DataGroup.from_files(game, files, 1 - test_fraction, 1)

    train_sampler = PositionSampler(train_group, batch_size, None, include_final, False, train_random_symmetries,
                                    threads=2)
    test_sampler = PositionSampler(test_group, batch_size, None, include_final, False, train_random_symmetries,
                                   threads=1)

    example_input = train_sampler.next_batch().input_full.to("cpu")
    print("Tracing model")
    network_jit = torch.jit.trace(initial_network(), example_input)
    print("Saving JIT")
    torch.jit.save(network_jit, "test.pt")
    print("Saving ONNX")
    torch.onnx.export(network_jit, example_input, "test.onnx")
    print("done saving model")
    return

    print(f"File count: {len(files)}")
    print(f"  Train simulation count: {len(train_group.simulations)}")
    print(f"  Train position count: {len(train_group.positions)}")
    print(f"  Test simulation count: {len(test_group.simulations)}")
    print(f"  Test position count: {len(test_group.positions)}")

    last_bi = find_last_finished_batch(output_folder)

    if last_bi is None:
        logger = Logger()
        start_bi = 0
        network = initial_network()
    else:
        assert allow_resume, f"Not allowed to resume, but found existing batch {last_bi}"

        logger = Logger.load(os.path.join(output_folder, "log.npz"))
        start_bi = last_bi + 1
        network = torch.jit.load(os.path.join(output_folder, f"network_{last_bi}.pt"))

    network.to(DEVICE)
    print_param_count(network)

    # optimizer = SGD(network.parameters(), weight_decay=1e-5, lr=0.0, momentum=0.9)
    # schedule = WarmupSchedule(100, FixedSchedule([0.02, 0.01, 0.001], [900, 2_000]))

    # optimizer = torch.optim.AdamW(network.parameters(), weight_decay=1e-5)
    optimizer = torch.optim.AdamW(network.parameters(), weight_decay=1e-5)
    schedule = None

    plotter.set_title(f"supervised {output_folder}")
    plotter.set_can_pause(True)
    plotter.update(logger)

    supervised_loop(
        settings, schedule, optimizer,
        start_bi, output_folder, logger, plotter,
        network, train_sampler, test_sampler,
        test_steps, save_steps,
    )

    # currently, these never trigger (since the loop never stops), but that may change in the future
    train_sampler.close()
    test_sampler.close()


if __name__ == '__main__':
    run_with_plotter(main)
