use board_game::board::Board;
use serde::{Deserialize, Serialize};

use kz_core::zero::node::UctWeights;
use std::fmt::{Display, Formatter};

use crate::simulation::Simulation;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupSettings {
    pub game: String,
    pub muzero: bool,

    pub first_gen: u32,
    pub output_folder: String,
    pub games_per_gen: usize,

    // TODO implement some kind of adaptive batch sizing, especially for root
    pub cpu_threads_per_device: usize,
    pub gpu_threads_per_device: usize,
    pub gpu_batch_size: usize,
    pub gpu_batch_size_root: usize,

    pub saved_state_channels: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Command {
    StartupSettings(StartupSettings),
    NewSettings(Settings),
    NewNetwork(String),
    WaitForNewNetwork,
    Stop,
}

#[derive(Debug)]
pub enum GeneratorUpdate<B: Board> {
    Stop,

    StartedSimulation {
        generator_id: usize,
    },

    FinishedMove {
        generator_id: usize,
        curr_game_length: usize,
    },

    FinishedSimulation {
        generator_id: usize,
        simulation: Simulation<B>,
    },

    Evals {
        // the number of evaluations that hit the cache
        cached_evals: u64,
        // the number of (expand) evaluations that did not hit the cache
        real_evals: u64,
        // the number of root muzero evals
        root_evals: u64,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ServerUpdate {
    Stopped,
    FinishedFile { index: u32 },
}

//TODO split this into AlphaZero and MuZero structs, the overlap is getting pretty small
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    // self-play game affecting settings
    pub max_game_length: Option<u64>,
    pub weights: Weights,
    pub use_value: bool,

    pub random_symmetries: bool,

    pub temperature: f32,
    pub zero_temp_move_count: u32,

    pub dirichlet_alpha: f32,
    pub dirichlet_eps: f32,

    pub full_search_prob: f64,
    pub full_iterations: u64,
    pub part_iterations: u64,

    pub top_moves: usize,

    // performance
    pub cache_size: usize,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub struct Weights {
    pub exploration_weight: Option<f32>,
    pub moves_left_weight: Option<f32>,
    pub moves_left_clip: Option<f32>,
    pub moves_left_sharpness: Option<f32>,
}

impl Weights {
    pub fn to_uct(&self) -> UctWeights {
        let default = UctWeights::default();
        UctWeights {
            exploration_weight: self.exploration_weight.unwrap_or(default.exploration_weight),
            moves_left_weight: self.moves_left_weight.unwrap_or(default.moves_left_weight),
            moves_left_clip: self.moves_left_clip.unwrap_or(default.moves_left_clip),
            moves_left_sharpness: self.moves_left_sharpness.unwrap_or(default.moves_left_sharpness),
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub enum Game {
    TTT,
    STTT,
    Chess,
    ChessHist { length: usize },
    Ataxx { size: u8 },
}

impl Game {
    pub fn parse(str: &str) -> Option<Game> {
        match str {
            "ttt" => return Some(Game::TTT),
            "sttt" => return Some(Game::STTT),
            "chess" => return Some(Game::Chess),
            "ataxx" => return Some(Game::Ataxx { size: 7 }),
            _ => {}
        };

        if let Some(size) = str.strip_prefix("ataxx-") {
            let size: u8 = size.parse().ok()?;
            return Some(Game::Ataxx { size });
        }
        if let Some(length) = str.strip_prefix("chess-hist-") {
            let length: usize = length.parse().ok()?;
            return Some(Game::ChessHist { length });
        }

        None
    }
}

impl Display for Game {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Game::TTT => write!(f, "ttt"),
            Game::STTT => write!(f, "sttt"),
            Game::Chess => write!(f, "chess"),
            Game::ChessHist { length } => write!(f, "chess-hist-{}", length),
            Game::Ataxx { size } => write!(f, "ataxx-{}", size),
        }
    }
}
