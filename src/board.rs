#![allow(dead_code)]

use std::fmt::{self, Debug, Write};

use itertools::Itertools;
use rand::distributions::{Distribution, Standard};
use rand::Rng;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum Player {
    X,
    O,
    Neutral,
}

impl Player {
    pub fn other(self) -> Player {
        match self {
            Player::X => Player::O,
            Player::O => Player::X,
            Player::Neutral => Player::Neutral,
        }
    }

    pub fn sign(self) -> f32 {
        match self {
            Player::X => 1.0,
            Player::O => -1.0,
            Player::Neutral => 0.0,
        }
    }

    fn index(self) -> u32 {
        match self {
            Player::X => 0,
            Player::O => 1,
            Player::Neutral => panic!(),
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct Coord(u8);

impl Coord {
    pub fn all() -> impl Iterator<Item=Coord> {
        (0..81).map(|o| Self::from_o(o))
    }

    pub fn all_yx() -> impl Iterator<Item=Coord> {
        (0..81).map(|i| Self::from_xy(i % 9, i / 9))
    }

    pub fn from_oo(om: u8, os: u8) -> Coord {
        debug_assert!(om < 9);
        debug_assert!(os < 9);
        Coord(9 * om + os)
    }

    pub fn from_o(o: u8) -> Coord {
        debug_assert!(o < 81);
        Coord(o)
    }

    pub fn from_xy(x: u8, y: u8) -> Coord {
        debug_assert!(x < 9 && y < 9);
        Coord(((x / 3) + (y / 3) * 3) * 9 + ((x % 3) + (y % 3) * 3))
    }

    pub fn om(self) -> u8 {
        self.0 / 9
    }

    pub fn os(self) -> u8 {
        self.0 % 9
    }

    pub fn o(self) -> u8 {
        9 * self.om() + self.os()
    }

    pub fn yx(self) -> u8 {
        9 * self.y() + self.x()
    }

    pub fn x(self) -> u8 {
        (self.om() % 3) * 3 + (self.os() % 3)
    }

    pub fn y(self) -> u8 {
        (self.om() / 3) * 3 + (self.os() / 3)
    }
}

impl Debug for Coord {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "({}, {})", self.om(), self.os())
    }
}

//TODO implement simpler partialeq again
#[derive(Clone, Eq, PartialEq, Debug, Hash)]
pub struct Board {
    //TODO try u16 here, that makes Board a lot smaller and maybe even feasible to store in the tree?
    grids: [u32; 9],
    main_grid: u32,

    pub last_move: Option<Coord>,
    pub next_player: Player,
    pub won_by: Option<Player>,

    macro_mask: u32,
    macro_open: u32,
}

//TODO implement a size hint
//TODO look into other iterator speedup functions that can be implemented
pub struct BoardMoveIterator<'a> {
    board: &'a Board,
    macro_left: u32,
    curr_om: u32,
    grid_left: u32,
}

impl<'a> BoardMoveIterator<'a> {
    fn empty(board: &Board) -> BoardMoveIterator {
        BoardMoveIterator { board, macro_left: 0, curr_om: 0, grid_left: 0 }
    }
    fn new(board: &Board) -> BoardMoveIterator {
        BoardMoveIterator { board, macro_left: board.macro_mask, curr_om: 0, grid_left: 0 }
    }
}

impl<'a> Iterator for BoardMoveIterator<'a> {
    type Item = Coord;

    fn next(&mut self) -> Option<Coord> {
        if self.grid_left == 0 {
            if self.macro_left == 0 {
                return None;
            } else {
                self.curr_om = self.macro_left.trailing_zeros();
                self.macro_left &= self.macro_left - 1;
                self.grid_left = !compact_grid(self.board.grids[self.curr_om as usize]) & Board::FULL_MASK;
            }
        }

        let os = self.grid_left.trailing_zeros();
        self.grid_left &= self.grid_left - 1;

        Some(Coord::from_oo(self.curr_om as u8, os as u8))
    }
}

/// A symmetry group element for Board transformations. Can represent any combination of
/// flips, rotating and transposing, which result in 8 distinct elements.
///
/// The `Default::default()` value means no transformation.
///
/// The internal representation is such that first x and y are transposed,
/// then each axis is flipped separately.
#[derive(Debug, Copy, Clone)]
pub struct Symmetry {
    pub transpose: bool,
    pub flip_x: bool,
    pub flip_y: bool,
}

impl Default for Symmetry {
    fn default() -> Self {
        Symmetry { transpose: false, flip_x: false, flip_y: false }
    }
}

impl Distribution<Symmetry> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> Symmetry {
        Symmetry { transpose: rng.gen(), flip_x: rng.gen(), flip_y: rng.gen() }
    }
}

impl Symmetry {
    pub fn all() -> impl Iterator<Item=Symmetry> {
        (0..8).map(|i| Symmetry {
            transpose: i & 0b100 != 0,
            flip_x: i & 0b010 != 0,
            flip_y: i & 0b001 != 0,
        })
    }

    pub fn inverse(self) -> Symmetry {
        Symmetry {
            transpose: self.transpose,
            flip_x: if self.transpose { self.flip_y } else { self.flip_x },
            flip_y: if self.transpose { self.flip_x } else { self.flip_y },
        }
    }

    pub fn map_coord(self, coord: Coord) -> Coord {
        Coord::from_oo(self.map_oo(coord.om()), self.map_oo(coord.os()))
    }

    pub fn map_oo(self, oo: u8) -> u8 {
        let (mut x, mut y) = (oo % 3, oo / 3);
        if self.transpose { std::mem::swap(&mut x, &mut y) };
        if self.flip_x { x = 2 - x };
        if self.flip_y { y = 2 - y };
        x + y * 3
    }

    fn map_grid(self, grid: u32) -> u32 {
        let mut result = 0;
        for oo_input in 0..9 {
            let oo_result = self.map_oo(oo_input);
            let get = (grid >> oo_input) & 0b1_000_000_001;
            result |= get << oo_result;
        }
        result
    }
}

impl Board {
    pub const MAX_AVAILABLE_MOVES: u32 = 9 * 9;

    const FULL_MASK: u32 = 0b111_111_111;

    pub fn new() -> Board {
        Board {
            grids: [0; 9],
            main_grid: 0,
            last_move: None,
            next_player: Player::X,
            won_by: None,
            macro_mask: Board::FULL_MASK,
            macro_open: Board::FULL_MASK,
        }
    }

    pub fn is_done(&self) -> bool {
        self.won_by != None
    }

    pub fn tile(&self, coord: Coord) -> Player {
        get_player(self.grids[coord.om() as usize], coord.os())
    }

    pub fn macr(&self, om: u8) -> Player {
        debug_assert!(om < 9);
        get_player(self.main_grid, om)
    }

    pub fn map_symmetry(&self, sym: Symmetry) -> Board {
        let mut grids = [0; 9];
        for oo in 0..9 {
            grids[sym.map_oo(oo) as usize] = sym.map_grid(self.grids[oo as usize])
        }

        Board {
            grids,
            main_grid: 0,
            last_move: self.last_move.map(|c| sym.map_coord(c)),
            next_player: self.next_player,
            won_by: self.won_by,
            macro_mask: sym.map_grid(self.macro_mask),
            macro_open: sym.map_grid(self.macro_open),
        }
    }

    /// Return the number of non-empty tiles.
    pub fn count_tiles(&self) -> u32 {
        self.grids.iter().map(|tile| tile.count_ones()).sum()
    }

    pub fn available_moves(&self) -> impl Iterator<Item=Coord> + '_ {
        return if self.is_done() {
            BoardMoveIterator::empty(&self)
        } else {
            BoardMoveIterator::new(&self)
        };
    }

    pub fn random_available_move<R: Rng>(&self, rand: &mut R) -> Option<Coord> {
        if self.is_done() {
            return None;
        }

        let mut count = 0;
        for om in BitIter::of(self.macro_mask) {
            count += 9 - self.grids[om as usize].count_ones();
        }

        let mut index = rand.gen_range(0..count);

        for om in BitIter::of(self.macro_mask) {
            let grid = self.grids[om as usize];
            let grid_count = 9 - grid.count_ones();

            if index < grid_count {
                let os = get_nth_set_bit(!compact_grid(grid), index as u32);
                return Some(Coord::from_oo(om as u8, os as u8));
            }

            index -= grid_count;
        }

        //todo try unchecked here
        unreachable!()
    }

    pub fn is_available_move(&self, coord: Coord) -> bool {
        let om = coord.om();
        let os = coord.os();
        has_bit(self.macro_mask, om) &&
            !has_bit(compact_grid(self.grids[om as usize]), os)
    }

    pub fn clone_and_play(&self, coord: Coord) -> Board {
        let mut next = self.clone();
        next.play(coord);
        next
    }

    pub fn play(&mut self, coord: Coord) -> bool {
        debug_assert!(!self.is_done(), "can't play on done board");
        debug_assert!(self.is_available_move(coord), "move not available");

        //do actual move
        let won_grid = self.set_tile_and_update(self.next_player, coord);

        //update for next player
        self.last_move = Some(coord);
        self.next_player = self.next_player.other();

        won_grid
    }

    fn set_tile_and_update(&mut self, player: Player, coord: Coord) -> bool {
        let om = coord.om();
        let os = coord.os();
        let p = (9 * player.index()) as u8;

        //set tile and macro, check win
        let new_grid = self.grids[om as usize] | (1 << (os + p));
        self.grids[om as usize] = new_grid;

        let grid_win = is_win_grid((new_grid >> p) & Board::FULL_MASK);
        if grid_win {
            let new_main_grid = self.main_grid | (1 << (om + p));
            self.main_grid = new_main_grid;

            if is_win_grid((new_main_grid >> p) & Board::FULL_MASK) {
                self.won_by = Some(player);
            }
        }

        //update macro masks, remove bit from open and recalculate mask
        if grid_win || new_grid.count_ones() == 9 {
            self.macro_open &= !(1 << om);
            if self.macro_open == 0 && self.won_by.is_none() {
                self.won_by = Some(Player::Neutral);
            }
        }
        self.macro_mask = self.calc_macro_mask(os);

        grid_win
    }

    fn calc_macro_mask(&self, os: u8) -> u32 {
        if has_bit(self.macro_open, os) {
            1u32 << os
        } else {
            self.macro_open
        }
    }
}

impl Default for Board {
    fn default() -> Self {
        Board::new()
    }
}

fn is_win_grid(grid: u32) -> bool {
    debug_assert!(has_mask(Board::FULL_MASK, grid));

    const WIN_GRIDS: [u32; 16] = [
        2155905152, 4286611584, 4210076288, 4293962368,
        3435954304, 4291592320, 4277971584, 4294748800,
        2863300736, 4294635760, 4210731648, 4294638320,
        4008607872, 4294897904, 4294967295, 4294967295
    ];
    has_bit(WIN_GRIDS[(grid / 32) as usize], (grid % 32) as u8)
}

fn has_bit(x: u32, i: u8) -> bool {
    ((x >> i) & 1) != 0
}

fn has_mask(x: u32, mask: u32) -> bool {
    x & mask == mask
}

fn get_nth_set_bit(mut x: u32, n: u32) -> u32 {
    for _ in 0..n {
        x &= x.wrapping_sub(1);
    }
    x.trailing_zeros()
}

fn compact_grid(grid: u32) -> u32 {
    (grid | grid >> 9) & Board::FULL_MASK
}

fn get_player(grid: u32, index: u8) -> Player {
    if has_bit(grid, index) {
        Player::X
    } else if has_bit(grid, index + 9) {
        Player::O
    } else {
        Player::Neutral
    }
}

struct BitIter {
    left: u32,
}

impl BitIter {
    fn of(int: u32) -> BitIter {
        BitIter { left: int }
    }
}

impl Iterator for BitIter {
    type Item = u32;

    fn next(&mut self) -> Option<<Self as Iterator>::Item> {
        if self.left == 0 {
            None
        } else {
            let index = self.left.trailing_zeros();
            self.left &= self.left - 1;
            Some(index)
        }
    }
}

fn symbol_from_tile(board: &Board, coord: Coord) -> char {
    let is_last = Some(coord) == board.last_move;
    let is_available = board.is_available_move(coord);

    let tuple = (is_available, is_last, board.tile(coord));
    match tuple {
        (false, false, Player::X) => 'x',
        (false, true, Player::X) => 'X',
        (false, false, Player::O) => 'o',
        (false, true, Player::O) => 'O',
        (true, false, Player::Neutral) => '.',
        (false, false, Player::Neutral) => ' ',
        _ => unreachable!("Invalid tile state {:?}", tuple)
    }
}

fn symbol_to_tile(c: char) -> (bool, bool, Player) {
    match c {
        'X' => (false, false, Player::X),
        'x' => (false, true, Player::X),
        'O' => (false, false, Player::O),
        'o' => (false, true, Player::O),
        ' ' => (false, false, Player::Neutral),
        '.' => (true, false, Player::Neutral),
        _ => panic!("unexpected character '{}'", c)
    }
}

impl fmt::Display for Board {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for y in 0..9 {
            if y == 3 || y == 6 {
                f.write_str("---+---+---\n")?;
            }

            for x in 0..9 {
                if x == 3 || x == 6 {
                    f.write_char('|')?;
                }
                f.write_char(symbol_from_tile(self, Coord::from_xy(x, y)))?;
            }

            f.write_char('\n')?;
        }

        Ok(())
    }
}

pub fn board_to_compact_string(board: &Board) -> String {
    Coord::all().map(|coord| symbol_from_tile(board, coord)).join("")
}

pub fn board_from_compact_string(s: &str) -> Board {
    assert!(s.chars().count() == 81, "compact string should have length 81");

    let mut board = Board::new();
    let mut last_move = None;

    for (o, c) in s.chars().enumerate() {
        let coord = Coord::from_o(o as u8);
        let (_, last, player) = symbol_to_tile(c);

        if last {
            last_move = Some((player, coord));
        }

        if player != Player::Neutral {
            board.set_tile_and_update(player, coord);
        }
    }

    if let Some((last_player, last_coord)) = last_move {
        board.set_tile_and_update(last_player, last_coord);
        board.last_move = Some(last_coord);
        board.next_player = last_player.other()
    }

    board
}

#[cfg(test)]
mod test {
    use itertools::Itertools;
    use rand::rngs::SmallRng;
    use rand::SeedableRng;
    use rand::seq::SliceRandom;

    use crate::board::{Board, Coord, Symmetry};
    use crate::board_gen::random_board_with_moves;

    #[test]
    fn test_random_distribution() {
        let mut board = Board::new();
        let mut rand = SmallRng::seed_from_u64(0);

        while !board.is_done() {
            let moves: Vec<Coord> = board.available_moves().collect();

            let mut counts: [i32; 81] = [0; 81];
            for _ in 0..1_000_000 {
                counts[board.random_available_move(&mut rand).unwrap().o() as usize] += 1;
            }

            let avg = (1_000_000 / moves.len()) as i32;

            for (mv, &count) in counts.iter().enumerate() {
                if moves.contains(&Coord::from_o(mv as u8)) {
                    debug_assert!((count.wrapping_sub(avg)).abs() < 10_000, "uniformly distributed")
                } else {
                    assert_eq!(count, 0, "only actual moves returned")
                }
            }

            let mv = moves.choose(&mut rand).unwrap().o();
            board.play(Coord::from_o(mv as u8));
        }
    }

    #[test]
    fn symmetries() {
        let mut rng = SmallRng::seed_from_u64(5);
        let board = random_board_with_moves(10, &mut rng);
        println!("Original:\n{}", board);

        for i in 0..8 {
            let sym = Symmetry {
                transpose: i & 0b001 != 0,
                flip_x: i & 0b010 != 0,
                flip_y: i & 0b100 != 0,
            };
            let sym_inv = sym.inverse();

            println!("{:?}", sym);
            println!("inverse: {:?}", sym_inv);

            let mapped = board.map_symmetry(sym);
            let back = mapped.map_symmetry(sym_inv);

            // these prints test that the board is consistent enough to print it
            println!("Mapped:\n{}", mapped);
            println!("Back:\n{}", back);

            if i == 0 {
                assert_eq!(board, mapped);
            }
            assert_eq!(board, back);

            let expected_moves = board.available_moves().map(|c| sym.map_coord(c)).sorted_by_key(|c| c.o()).collect_vec();
            let actual_moves = mapped.available_moves().sorted_by_key(|c| c.o()).collect_vec();
            assert_eq!(expected_moves, actual_moves);
        }
    }
}