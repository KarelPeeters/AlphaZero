from dataclasses import dataclass, field
from typing import Tuple


@dataclass
class Game:
    name: str

    board_size: int
    input_bool_channels: int
    input_scalar_channels: int
    policy_channels: int

    average_game_length: float
    average_available_moves: float

    input_bool_shape: Tuple[int, int, int] = field(init=False)
    input_scalar_shape: Tuple[int, int, int] = field(init=False)

    full_input_channels: int = field(init=False)
    full_input_shape: Tuple[int, int, int] = field(init=False)
    policy_shape: Tuple[int, int, int] = field(init=False)

    def __post_init__(self):
        self.input_bool_shape = (self.input_bool_channels, self.board_size, self.board_size)
        self.input_scalar_shape = (self.input_scalar_channels, self.board_size, self.board_size)

        self.full_input_channels = self.input_bool_channels + self.input_scalar_channels
        self.full_input_shape = (self.full_input_channels, self.board_size, self.board_size)
        self.policy_shape = (self.policy_channels, self.board_size, self.board_size)

    @classmethod
    def find(cls, name: str):
        for game in GAMES:
            if game.name == name:
                return game
        raise KeyError("Game '{}' not found", name)


GAMES = [
    Game(
        name="ataxx",
        board_size=7,
        input_bool_channels=3,
        input_scalar_channels=0,
        policy_channels=17,

        average_game_length=150,
        average_available_moves=40,
    ),
    Game(
        name="chess",
        board_size=8,
        input_bool_channels=13,
        input_scalar_channels=8,
        policy_channels=73,

        average_game_length=150,
        average_available_moves=35,
    ),
    Game(
        name="sttt",
        board_size=9,
        input_bool_channels=3,
        input_scalar_channels=0,
        policy_channels=1,

        average_game_length=50,
        average_available_moves=10,
    ),
    Game(
        name="ttt",
        board_size=3,
        input_bool_channels=2,
        input_scalar_channels=0,
        policy_channels=1,

        average_game_length=7,
        average_available_moves=6,
    )
]
