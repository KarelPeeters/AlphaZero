import itertools
import os

import torch.jit
from torch import nn
from torch.optim import AdamW

from lib.data.file import DataFile
from lib.data.group import DataGroup
from lib.data.sampler import PositionSampler
from lib.games import Game
from lib.logger import Logger
from lib.model.layers import Flip
from lib.model.post_act import ResTower, ConcatInputsChannelwise, PredictionHeads, ScalarHead, DensePolicyHead
from lib.networks import MuZeroNetworks
from lib.plotter import run_with_plotter, LogPlotter
from lib.train import TrainSettings, ScalarTarget
from lib.util import DEVICE


def main(plotter: LogPlotter):
    print(f"Using device {DEVICE}")

    game = Game.find("chess")

    paths = [
        fr"C:\Documents\Programming\STTT\kZero\data\loop\chess\run-accel\selfplay\games_{gi}.json"
        for gi in range(800, 1200)
    ]

    files = [DataFile.open(game, p) for p in paths]
    group = DataGroup.from_files(game, files)
    sampler = PositionSampler(
        group,
        batch_size=128,
        unroll_steps=5,
        include_final=True,
        include_final_for_each=False,
        threads=1,
    )

    train = TrainSettings(
        game=game,
        value_weight=0.1,
        wdl_weight=1.0,
        policy_weight=1.0,
        sim_weight=0.01,
        moves_left_delta=20,
        moves_left_weight=0.0001,
        clip_norm=4.0,
        scalar_target=ScalarTarget.Final,
        train_in_eval_mode=False,
        mask_policy=False,
    )

    output_path = "../../data/muzero/restart-sim"
    os.makedirs(output_path, exist_ok=False)

    channels = 128
    depth = 16

    saved_channels = 128
    state_quant_bits = None

    # TODO affine or not
    # TODO attention policy head? maybe (mult with) fully connected instead

    representation = nn.Sequential(
        ResTower(depth, game.full_input_channels, channels, final_affine=True),
        nn.Hardtanh(-1.0, 1.0),
    )
    dynamics = ConcatInputsChannelwise(nn.Sequential(
        ResTower(depth, saved_channels + game.input_mv_channels, channels, final_affine=True),
        nn.Hardtanh(-1.0, 1.0),
        Flip(dim=2),
    ))
    prediction = PredictionHeads(
        common=ResTower(depth, saved_channels, channels, final_affine=True),
        scalar_head=ScalarHead(game.board_size, channels, 8, 128),
        policy_head=DensePolicyHead(game, channels, 32, None)
    )

    networks = MuZeroNetworks(
        state_channels=channels,
        state_channels_saved=saved_channels,
        state_quant_bits=state_quant_bits,
        representation=representation,
        dynamics=dynamics,
        prediction=prediction,
    )
    networks.to(DEVICE)

    logger = Logger()
    optimizer = AdamW(networks.parameters(), weight_decay=1e-5)

    plotter.set_can_pause(True)

    print("Start training")
    for bi in itertools.count():
        if bi % 100 == 0:
            logger.save(f"{output_path}/log.npz")
        if bi % 500 == 0:
            torch.jit.save(torch.jit.script(networks), f"{output_path}/models_{bi}.pb")

        plotter.block_while_paused()
        print(f"bi: {bi}")
        logger.start_batch()

        batch = sampler.next_unrolled_batch()
        train.train_step(batch, networks, optimizer, logger)

        plotter.update(logger)


if __name__ == '__main__':
    run_with_plotter(main)
