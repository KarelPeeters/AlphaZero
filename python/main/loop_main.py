import glob
import sys

from torch.optim import AdamW

from lib.data.file import DataFile
from lib.games import Game
from lib.loop import FixedSelfplaySettings, LoopSettings
from lib.model.post_act import PostActNetwork, PostActScalarHead, PostActAttentionPolicyHead
from lib.selfplay_client import SelfplaySettings, UctWeights
from lib.train import TrainSettings, ScalarTarget


def main():
    game = Game.find("chess")

    fixed_settings = FixedSelfplaySettings(
        game=game,
        threads_per_device=2,
        batch_size=128,
        games_per_gen=200,
        reorder_games=False,
    )

    selfplay_settings = SelfplaySettings(
        temperature=1.0,
        zero_temp_move_count=1000,
        use_value=False,
        max_game_length=400,
        keep_tree=False,
        dirichlet_alpha=0.2,
        dirichlet_eps=0.25,
        full_search_prob=1.0,
        full_iterations=200,
        part_iterations=20,
        weights=UctWeights.default(),
        random_symmetries=True,
        cache_size=200,
    )

    train_settings = TrainSettings(
        game=game,
        value_weight=0.1,
        wdl_weight=1.0,
        policy_weight=1.0,
        moves_left_delta=20,
        moves_left_weight=0.0001,
        clip_norm=20.0,
        value_target=ScalarTarget.Final,
        train_in_eval_mode=False,
    )

    def dummy_network():
        return PostActNetwork(
            game, 1, 8,
            PostActScalarHead(game, 8, 2, 16),
            PostActAttentionPolicyHead(game, 8, 4),
        )

    def initial_network():
        channels = 128
        return PostActNetwork(
            game, 16, channels,
            PostActScalarHead(game, channels, 8, 128),
            PostActAttentionPolicyHead(game, channels, channels),
        )

    initial_files_pattern = ""

    # TODO implement retain setting, maybe with a separate training folder even
    settings = LoopSettings(
        gui=sys.platform == "win32",
        root_path=f"data/loop/{game.name}/16x128/",

        dummy_network=dummy_network,
        initial_network=initial_network,
        initial_data_files=[DataFile.open(game, path) for path in glob.glob(initial_files_pattern)],

        only_generate=False,

        min_buffer_size=500_000,
        max_buffer_size=1_000_000,

        train_batch_size=128,
        samples_per_position=0.3,

        optimizer=lambda params: AdamW(params, weight_decay=1e-3),

        fixed_settings=fixed_settings,
        selfplay_settings=selfplay_settings,
        train_settings=train_settings,
    )

    settings.calc_batch_count_per_gen()
    settings.run_loop()


if __name__ == '__main__':
    main()
