import itertools
import os

import torch.jit
from torch import nn
from torch.optim import AdamW

from lib.data.buffer import FileListSampler
from lib.data.file import DataFile
from lib.games import Game
from lib.logger import Logger
from lib.model.layers import Flip
from lib.model.post_act import ResTower, ConcatInputsChannelwise, PredictionHeads, ScalarHead, AttentionPolicyHead, \
    ResBlock
from lib.networks import MuZeroNetworks
from lib.plotter import run_with_plotter, LogPlotter
from lib.train import TrainSettings, ScalarTarget
from lib.util import DEVICE


def main(plotter: LogPlotter):
    print(f"Using device {DEVICE}")

    game = Game.find("chess")

    paths = [
        fr"C:\Documents\Programming\STTT\AlphaZero\data\loop\chess\16x128\selfplay\games_{i}.bin"
        for i in range(2600, 3600)
    ]
    files = [DataFile.open(game, p) for p in paths]

    sampler = FileListSampler(game, files, batch_size=64, unroll_steps=5, threads=1)

    train = TrainSettings(
        game=game,
        value_weight=0.1,
        wdl_weight=1.0,
        policy_weight=1.0,
        moves_left_delta=20,
        moves_left_weight=0.0001,
        clip_norm=4.0,
        scalar_target=ScalarTarget.Final,
        train_in_eval_mode=False,
        mask_policy_root=False,
        mask_policy_unrolled=False,
        root_policy_scale=1.0,
    )

    output_path = "../../data/muzero/limit64_reshead"
    os.makedirs(output_path, exist_ok=False)

    channels = 256
    depth = 16
    saved_channels = 64

    representation = nn.Sequential(
        ResTower(depth, game.full_input_channels, channels, final_affine=False),
        nn.Hardtanh(-1.0, 1.0),
    )
    dynamics = ConcatInputsChannelwise(nn.Sequential(
        ResTower(depth, saved_channels + game.input_mv_channels, channels, final_affine=False),
        nn.Hardtanh(-1.0, 1.0),
        Flip(dim=2),
    ))
    prediction = PredictionHeads(
        common=ResBlock(channels),
        scalar_head=ScalarHead(game.board_size, channels, 8, 128),
        policy_head=AttentionPolicyHead(game, channels, 64)
    )

    networks = MuZeroNetworks(
        state_channels=channels,
        state_channels_saved=saved_channels,
        state_quant_bits=8,
        representation=representation,
        dynamics=dynamics,
        prediction=prediction,
    )
    networks.to(DEVICE)

    logger = Logger()
    optimizer = AdamW(networks.parameters(), weight_decay=1e-5)

    print("Start training")
    for bi in itertools.count():
        if bi % 100 == 0:
            logger.save(f"{output_path}/log.npz")
        if bi % 500 == 0:
            torch.jit.save(torch.jit.script(networks), f"{output_path}/models_{bi}.pb")

        print(f"bi: {bi}")
        logger.start_batch()

        batch = sampler.next_unrolled_batch()
        train.train_step_unrolled(batch, networks, optimizer, logger)

        plotter.update(logger)


if __name__ == '__main__':
    run_with_plotter(main)
