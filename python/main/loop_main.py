import sys

import torch
from torch.optim import AdamW, SGD

from lib.games import Game
from lib.loop import FixedSelfplaySettings, LoopSettings
from lib.model.lc0_pre_act import LCZOldPreNetwork
from lib.model.post_act import PostActNetwork, PostActValueHead, PostActAttentionPolicyHead
from lib.model.simple import DenseNetwork
from lib.selfplay_client import SelfplaySettings
from lib.train import TrainSettings, ValueTarget


def main():
    game = Game.find("chess")

    fixed_settings = FixedSelfplaySettings(
        game=game,
        threads_per_device=2,
        batch_size=512,
        games_per_gen=1000,
        reorder_games=False,
    )

    selfplay_settings = SelfplaySettings(
        temperature=1.0,
        zero_temp_move_count=30,
        use_value=False,
        max_game_length=300,
        keep_tree=False,
        dirichlet_alpha=0.2,
        dirichlet_eps=0.25,
        full_search_prob=1.0,
        full_iterations=600,
        part_iterations=20,
        exploration_weight=2.0,
        random_symmetries=True,
        cache_size=600,
    )

    train_settings = TrainSettings(
        game=game,
        value_weight=0.1,
        wdl_weight=1.0,
        policy_weight=1.0,
        clip_norm=20.0,
        value_target=ValueTarget.Final,
        train_in_eval_mode=False,
    )

    def initial_network():
        return torch.jit.load("data/network_24448.pb")

    # TODO implement retain setting, maybe with a separate training folder even
    settings = LoopSettings(
        gui=sys.platform == "win32",
        root_path=f"data/loop/{game.name}/small/",
        initial_network=initial_network,
        only_generate=True,

        target_buffer_size=1_000_000,
        train_steps_per_gen=4,
        train_batch_size=256,

        optimizer=lambda params: SGD(params, lr=0.01, momentum=0.9, weight_decay=1e-5),

        fixed_settings=fixed_settings,
        selfplay_settings=selfplay_settings,
        train_settings=train_settings,
    )

    print_expected_buffer_behaviour(settings, game.estimate_moves_per_game)

    settings.run_loop()


def print_expected_buffer_behaviour(settings: LoopSettings, average_game_length: int):
    games_in_buffer = settings.target_buffer_size / average_game_length
    gens_in_buffer = games_in_buffer / settings.fixed_settings.games_per_gen

    positions_per_gen = settings.train_steps_per_gen * settings.train_batch_size
    visits_per_position = gens_in_buffer * positions_per_gen / settings.target_buffer_size
    visits_per_game = visits_per_position * average_game_length

    print("Expected numbers:")
    print(f"  Positions in buffer: {settings.target_buffer_size}")
    print(f"  Games in buffer: {games_in_buffer}")
    print(f"  Generations in buffer: {gens_in_buffer}")
    print(f"  Positions per gen: {positions_per_gen}")
    print(f"  Visits per position: {visits_per_position}")
    print(f"  Visits per game: {visits_per_game}")


if __name__ == '__main__':
    main()
