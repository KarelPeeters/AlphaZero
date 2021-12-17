import itertools
import os
import time
from dataclasses import dataclass
from multiprocessing.pool import ThreadPool
from pathlib import Path
from threading import Thread
from typing import Callable, Optional, Tuple, Iterator, List

import torch
from torch import nn
from torch.optim import Optimizer

from lib.data.buffer import FileListSampler
from lib.data.file import DataFile
from lib.games import Game
from lib.logger import Logger
from lib.plotter import LogPlotter, qt_app
from lib.save_onnx import save_onnx
from lib.selfplay_client import SelfplaySettings, StartupSettings, SelfplayClient
from lib.train import TrainSettings
from lib.util import DEVICE, print_param_count
from main.write_test_networks import CHECK_BATCH_SIZE


@dataclass
class FixedSelfplaySettings:
    game: Game
    threads_per_device: int
    batch_size: int
    games_per_gen: int
    reorder_games: bool

    def to_startup(self, output_folder: str, first_gen: int):
        return StartupSettings(
            output_folder=output_folder,
            first_gen=first_gen,
            game=self.game.name,
            threads_per_device=self.threads_per_device,
            batch_size=self.batch_size,
            games_per_gen=self.games_per_gen,
            reorder_games=self.reorder_games,
        )


@dataclass
class LoopSettings:
    gui: bool
    root_path: str
    initial_network: Callable[[], nn.Module]
    only_generate: bool

    target_buffer_size: int
    train_steps_per_gen: int

    # TODO re-implement testing
    # test_fraction: float
    # eval_steps_per_gen: int

    optimizer: Callable[[Iterator[nn.Parameter]], Optimizer]
    train_batch_size: int

    fixed_settings: FixedSelfplaySettings
    selfplay_settings: Optional[SelfplaySettings]
    train_settings: TrainSettings

    # TODO compact these properties somehow
    @property
    def initial_network_path_onnx(self):
        return os.path.join(self.root_path, "initial_network.onnx")

    @property
    def log_path(self):
        return os.path.join(self.root_path, "log.npz")

    @property
    def selfplay_path(self):
        return os.path.join(self.root_path, "selfplay")

    @property
    def training_path(self):
        return os.path.join(self.root_path, "training")

    def run_loop(self):
        print(f"Starting loop with cwd {os.getcwd()}")
        assert os.path.exists("./rust") and os.path.exists("./python"), \
            f"Should be run in root kZero folder, got {os.getcwd()}"

        os.makedirs(self.selfplay_path, exist_ok=True)
        os.makedirs(self.training_path, exist_ok=True)

        start_gen, buffer, logger, network, network_path_onnx = self.load_start_state()
        print_param_count(network)

        if self.gui:
            app = qt_app()
            plotter = LogPlotter()
            plotter.update(logger)
        else:
            app = None
            plotter = None

        args = (start_gen, buffer, logger, plotter, network, network_path_onnx)

        if self.gui:
            thread = Thread(target=self.run_loop_thread, args=args)
            thread.start()
            app.exec()
        else:
            self.run_loop_thread(*args)

    def run_loop_thread(
            self,
            start_gen: 'Generation', buffer: 'LoopBuffer',
            logger: Logger, plotter: Optional[LogPlotter],
            network: nn.Module, network_path_onnx: str
    ):
        game = self.fixed_settings.game
        optimizer = self.optimizer(network.parameters())

        startup_settings = self.fixed_settings.to_startup(
            output_folder=self.selfplay_path,
            first_gen=start_gen.gi,
        )

        client = SelfplayClient()
        client.send_startup_settings(startup_settings)
        client.send_new_settings(self.selfplay_settings)
        client.send_new_network(network_path_onnx)

        for gi in itertools.count(start_gen.gi):
            logger.start_batch()
            logger.log("info", "gen", gi)

            print(f"Waiting for gen {gi} games")
            gen_start = time.perf_counter()
            actual_gi = client.wait_for_file()
            logger.log("time", "selfplay", time.perf_counter() - gen_start)
            assert gi == actual_gi, f"Unexpected finished generation, expected {gi} got {actual_gi}"

            if self.only_generate:
                print("Not training new network, we're only generating data")
                continue

            client.send_wait_for_new_network()

            gen = Generation.from_gi(self, gi)
            os.makedirs(gen.train_path, exist_ok=True)

            buffer.append(logger, DataFile.open(game, gen.games_path))
            self.evaluate_network(buffer, logger, network)

            train_sampler = buffer.sampler_full(self.train_batch_size)
            print(f"Training network on buffer with size {len(train_sampler)}")
            train_start = time.perf_counter()

            for bi in range(self.train_steps_per_gen):
                if bi != 0:
                    logger.start_batch()

                self.train_settings.train_step(train_sampler, network, optimizer, logger)
            train_sampler.close()

            logger.log("time", "train", time.perf_counter() - train_start)

            torch.jit.save(network, gen.network_path_pt)
            save_onnx(game, gen.network_path_onnx, network, CHECK_BATCH_SIZE)
            client.send_new_network(gen.network_path_onnx)

            logger.save(self.log_path)
            Path(gen.finished_path).touch()

            if plotter is not None:
                plotter.update(logger)

    def load_start_state(self) -> Tuple['Generation', 'LoopBuffer', Logger, nn.Module, str]:
        game = self.fixed_settings.game
        buffer = LoopBuffer(game, self.target_buffer_size)

        for gi in itertools.count():
            gen = Generation.from_gi(self, gi)
            prev = gen.prev

            if not os.path.exists(gen.finished_path):
                if prev is None:
                    print("Starting new run")
                    logger = Logger()

                    network = torch.jit.script(self.initial_network())
                    network.to(DEVICE)

                    prev_network_path_onnx = self.initial_network_path_onnx
                    save_onnx(game, prev_network_path_onnx, network, CHECK_BATCH_SIZE)
                else:
                    print(f"Continuing run, first gen {gi}")
                    logger = Logger.load(self.log_path)

                    network = torch.jit.load(prev.network_path_pt)
                    network.to(DEVICE)

                    prev_network_path_onnx = prev.network_path_onnx

                return gen, buffer, logger, network, prev_network_path_onnx

            print(f"Found finished generation {gi}")
            buffer.append(None, DataFile.open(game, gen.games_path))

    def evaluate_network(self, buffer: 'LoopBuffer', logger: Logger, network: nn.Module):
        setups = [
            ("eval-test-buffer", buffer.sampler_full(self.train_batch_size)),
            ("eval-test-last", buffer.sampler_last(self.train_batch_size)),
        ]

        network.eval()
        for prefix, sampler in setups:
            batch = sampler.next_batch()
            self.train_settings.evaluate_batch(network, prefix, logger, batch, self.train_settings.value_target)
            sampler.close()


@dataclass
class Generation:
    settings: 'LoopSettings'
    gi: int
    games_path: str
    train_path: str
    network_path_pt: str
    network_path_onnx: str
    finished_path: str

    @classmethod
    def from_gi(cls, settings: 'LoopSettings', gi: int):
        games_path = os.path.join(settings.selfplay_path, f"games_{gi}")
        train_path = os.path.join(settings.training_path, f"gen_{gi}")

        return Generation(
            settings=settings,
            gi=gi,
            games_path=games_path,
            train_path=train_path,
            network_path_pt=os.path.join(train_path, "network.pt"),
            network_path_onnx=os.path.join(train_path, "network.onnx"),
            finished_path=os.path.join(train_path, "finished.txt"),
        )

    @property
    def prev(self):
        if self.gi == 0:
            return None
        return Generation.from_gi(self.settings, self.gi - 1)


class LoopBuffer:
    def __init__(self, game: Game, target_positions: int):
        self.game = game
        self.pool = ThreadPool(2)
        self.target_positions = target_positions

        self.current_positions = 0
        self.files: List[DataFile] = []

    def append(self, logger: Optional[Logger], file: DataFile):
        self.files.append(file)
        self.current_positions += len(file)

        while self.current_positions - len(self.files[0]) > self.target_positions:
            self.current_positions -= len(self.files[0])
            del self.files[0]

        if logger:
            total_games = sum(f.info.game_count for f in self.files)

            logger.log("buffer", "gens", len(self.files))
            logger.log("buffer", "games", total_games)
            logger.log("buffer", "positions", self.current_positions)

            info = file.info

            logger.log("gen-size", "games", info.game_count)
            logger.log("gen-size", "positions", info.position_count)
            logger.log("gen-game-len", "game length min", info.min_game_length)
            logger.log("gen-game-len", "game length mean", info.position_count / info.game_count)
            logger.log("gen-game-len", "game length max", info.max_game_length)

            if info.root_wdl is not None:
                logger.log("gen-root-wdl", "w", info.root_wdl[0])
                logger.log("gen-root-wdl", "d", info.root_wdl[1])
                logger.log("gen-root-wdl", "l", info.root_wdl[2])

    def sampler_full(self, batch_size: int):
        return FileListSampler(self.game, self.files, batch_size)

    def sampler_last(self, batch_size: int):
        return FileListSampler(self.game, [self.files[-1]], batch_size)
