use std::cmp::{max, min, Reverse};
use std::collections::HashSet;

use board_game::board::{Board, Outcome, Player};
use board_game::games::ataxx::AtaxxBoard;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use decorum::N32;
use itertools::Itertools;
use rand::rngs::StdRng;
use rand::SeedableRng;
use tui::backend::CrosstermBackend;
use tui::buffer::Buffer;
use tui::layout::{Margin, Rect};
use tui::style::{Color, Modifier, Style};
use tui::widgets::Widget;
use tui::Terminal;

use cuda_nn_eval::Device;
use kz_core::mapping::ataxx::AtaxxStdMapper;
use kz_core::network::cudnn::CudaNetwork;
use kz_core::network::dummy::DummyNetwork;
use kz_core::zero::node::{Uct, UctWeights};
use kz_core::zero::step::FpuMode;
use kz_core::zero::tree::Tree;
use kz_core::zero::values::ZeroValuesAbs;
use kz_core::zero::wrapper::ZeroSettings;
use kz_util::display::display_option_empty;
use kz_util::throughput::PrintThroughput;
use nn_graph::onnx::load_graph_from_onnx_path;
use nn_graph::optimizer::optimize_graph;

#[derive(Debug)]
struct State<B: Board> {
    tree: Tree<B>,

    prev_nodes: Vec<RenderNode>,

    expanded_nodes: HashSet<usize>,
    selected_node: usize,

    view_offset: usize,
}

#[derive(Debug, Copy, Clone)]
struct RenderNode {
    depth: u32,
    node: usize,
}

fn main() -> std::io::Result<()> {
    let mut state = State {
        prev_nodes: vec![],
        tree: build_tree(true),
        expanded_nodes: HashSet::default(),
        selected_node: 0,
        view_offset: 0,
    };

    state.expanded_nodes.insert(0);

    // setup terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    loop {
        let mut prev_area = None;

        terminal.draw(|f| {
            let area = f.size().inner(&Margin {
                horizontal: 2,
                vertical: 2,
            });

            if area.area() > 0 {
                state.prepare_render(area);
                f.render_widget(&state, area);
            }

            prev_area = Some(area);
        })?;

        let event = crossterm::event::read()?;
        if event == Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::empty())) {
            break;
        }

        state.handle_event(prev_area.unwrap(), event);
    }

    // restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    Ok(())
}

const HEADER_SIZE: u16 = 2;
const OFFSET_MARGIN: usize = 3;
const COL_SPACING: u16 = 2;

impl<B: Board> State<B> {
    fn append_nodes(&self, curr: usize, depth: u32, result: &mut Vec<RenderNode>) {
        result.push(RenderNode { depth, node: curr });

        if self.expanded_nodes.contains(&curr) {
            if let Some(children) = self.tree[curr].children {
                let sorted_children = children
                    .iter()
                    .sorted_by_key(|&c| Reverse((self.tree[c].total_visits(), N32::from(self.tree[c].net_policy))));
                for c in sorted_children {
                    self.append_nodes(c, depth + 1, result);
                }
            }
        }
    }

    fn prepare_render(&mut self, area: Rect) {
        // collect nodes
        let mut nodes = std::mem::take(&mut self.prev_nodes);
        nodes.clear();
        self.append_nodes(0, 0, &mut nodes);
        self.prev_nodes = nodes;

        // fix offset
        let selected = self.selected_index();
        let margin = min(OFFSET_MARGIN, ((area.height - 1) / 2) as usize);
        let offset = (self.view_offset as i32).clamp(
            selected as i32 - (area.height as i32 - HEADER_SIZE as i32) + margin as i32,
            selected.saturating_sub(margin) as i32,
        );

        assert!(offset >= 0, "offset={}", offset);
        self.view_offset = offset as usize;
    }

    fn selected_index(&self) -> usize {
        self.prev_nodes
            .iter()
            .position(|n| n.node == self.selected_node)
            .unwrap()
    }

    fn handle_event(&mut self, area: Rect, event: Event) {
        match event {
            Event::Key(key) => match key.code {
                KeyCode::Up => {
                    let index = self.selected_index();
                    if index != 0 {
                        self.selected_node = self.prev_nodes[index - 1].node;
                    }
                }
                KeyCode::Down => {
                    self.selected_node = self
                        .prev_nodes
                        .get(self.selected_index() + 1)
                        .map_or(self.selected_node, |n| n.node);
                }
                KeyCode::Right => {
                    self.expanded_nodes.insert(self.selected_node);
                }
                KeyCode::Left => {
                    if self.expanded_nodes.contains(&self.selected_node) {
                        self.expanded_nodes.remove(&self.selected_node);
                    } else {
                        if let Some(parent) = self.tree[self.selected_node].parent {
                            self.selected_node = parent;
                            self.expanded_nodes.remove(&parent);
                        }
                    }
                }
                _ => (),
            },
            Event::Mouse(mouse) => {
                if mouse.kind == MouseEventKind::Up(MouseButton::Left) {
                    let i = mouse.row as i32 + self.view_offset as i32 - area.y as i32 - HEADER_SIZE as i32;

                    if i >= 0 {
                        if let Some(node) = self.prev_nodes.get(i as usize) {
                            self.selected_node = node.node;
                        }
                    }
                }
            }
            Event::Resize(_, _) => {}
        }
    }

    fn compute_col_starts(&self, area: Rect) -> (Vec<u16>, Vec<u16>) {
        let mut col_sizes = vec![0; 1 + COLUMN_INFO.len()];
        col_sizes[0] = 20;

        for (i, (n1, n2, _, _)) in COLUMN_INFO.iter().enumerate() {
            col_sizes[i] = max(col_sizes[i], max(n1.len(), n2.len()) as u16);
        }

        for &RenderNode { node, depth } in &self.prev_nodes {
            for (i, v) in self.column_values(node, depth).iter().enumerate() {
                col_sizes[i] = max(col_sizes[i], v.len() as u16);
            }
        }

        let col_starts = col_sizes
            .iter()
            .scan(area.x, |curr, &size| {
                *curr += size + COL_SPACING;
                Some(*curr - size - COL_SPACING)
            })
            .collect_vec();

        (col_sizes, col_starts)
    }

    fn column_values(&self, node: usize, depth: u32) -> Vec<String> {
        let node_index = node;
        let node = &self.tree[node];

        let arrow = if self.expanded_nodes.contains(&node_index) {
            "v"
        } else {
            ">"
        };

        //TODO convert to pov W D L again?
        let terminal = match node.outcome() {
            Err(_) => '?',
            Ok(None) => '.',
            Ok(Some(Outcome::WonBy(Player::A))) => 'A',
            Ok(Some(Outcome::Draw)) => 'D',
            Ok(Some(Outcome::WonBy(Player::B))) => 'B',
        };

        let mut result = vec![];

        result.push(format!("{:>2$} {}", arrow, node_index, (depth * 2) as usize));
        result.push(format!("{}", display_option_empty(node.last_move)));
        result.push(format!("{}", terminal));

        if node.virtual_visits == 0 {
            result.push(format!("{}", node.complete_visits));
        } else {
            result.push(format!("{} + {}", node.virtual_visits, node.complete_visits));
        }

        {
            // TODO convert to pov again?
            // TODO fix uct again
            let zero = node.values();
            let net = node.net_values.unwrap_or(ZeroValuesAbs::nan());
            let (uct, zero_policy) = if let Some(parent) = node.parent {
                let parent = &self.tree[parent];
                // let uct = node.uct(parent.total_visits(), parent.values(), false);
                let uct = Uct::nan();
                let zero_policy = node.complete_visits as f32 / (parent.complete_visits as f32 - 1.0);
                (uct, zero_policy)
            } else {
                (Uct::nan(), f32::NAN)
            };

            let values = [
                zero.wdl_abs.win_a,
                zero.wdl_abs.draw,
                zero.wdl_abs.win_b,
                zero.moves_left,
                zero_policy,
                net.wdl_abs.win_a,
                net.wdl_abs.draw,
                net.wdl_abs.win_b,
                net.moves_left,
                node.net_policy,
                uct.q,
                uct.u,
                uct.m,
            ];
            result.extend(
                values
                    .iter()
                    .map(|v| if v.is_nan() { "".to_owned() } else { format!("{:.3}", v) }),
            );
        }

        assert_eq!(result.len(), COLUMN_INFO.len());
        result
    }
}

const COLUMN_INFO: &[(&str, &str, bool, Color)] = &[
    ("Node", "", false, Color::Gray),
    ("Move", "", false, Color::Gray),
    ("T", "", false, Color::Gray),
    ("Visits", "", true, Color::Gray),
    ("Zero", "A", true, Color::Green),
    ("Zero", "D", true, Color::DarkGray),
    ("Zero", "B", true, Color::Red),
    ("Zero", "M", true, Color::Yellow),
    ("Zero", "P", true, Color::LightBlue),
    ("Net", "A", true, Color::Green),
    ("Net", "D", true, Color::DarkGray),
    ("Net", "B", true, Color::Red),
    ("Net", "M", true, Color::Yellow),
    ("Net", "P", true, Color::LightBlue),
    ("Uct", "Q", true, Color::Green),
    ("Uct", "U", true, Color::LightBlue),
    ("Uct", "M", true, Color::Yellow),
];

impl<B: Board> Widget for &State<B> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (col_sizes, col_starts) = self.compute_col_starts(area);

        for (i, &(n1, n2, _, color)) in COLUMN_INFO.iter().enumerate() {
            if i == 0 || COLUMN_INFO[i - 1].0 != n1 {
                buf.set_string(col_starts[i], area.y, n1, Style::default().fg(color));
            }
            buf.set_string(col_starts[i], area.y + 1, n2, Style::default().fg(color));
        }

        for y in 0..area.height - HEADER_SIZE {
            let full_y = area.y + y + HEADER_SIZE;
            let i = y as u32 + self.view_offset as u32;

            if let Some(&RenderNode { node, depth }) = self.prev_nodes.get(i as usize) {
                if node == self.selected_node {
                    let line = Rect::new(area.x, full_y, area.width, 1);
                    let style = Style::default().add_modifier(Modifier::REVERSED);
                    buf.set_style(line, style);
                }

                for (i, v) in self.column_values(node, depth).iter().enumerate() {
                    let just_right = COLUMN_INFO[i].2;
                    let color = COLUMN_INFO[i].3;

                    let x = if just_right {
                        col_starts[i] + (col_sizes[i] - v.len() as u16)
                    } else {
                        col_starts[i]
                    };

                    buf.set_string(x, full_y, v, Style::default().fg(color));
                }
            }
        }
    }
}

fn build_tree(real: bool) -> Tree<AtaxxBoard> {
    let batch_size = 1024;
    let settings = ZeroSettings::new(batch_size, UctWeights::default(), false, FpuMode::Relative(0.0), 1.0);
    let visits = 1_000_000;

    let board = AtaxxBoard::default();
    let path = r#"C:\Documents\Programming\STTT\kZero\data\networks\tmp\network_3874.onnx"#;
    let mapper = AtaxxStdMapper::new(board.size());

    // let board = AtaxxBoard::default();
    // let path = "C:/Documents/Programming/STTT/AlphaZero/data/loop/ataxx-7/16x128/training/gen_661/network.onnx";
    // let mapper = AtaxxStdMapper::new(board.size());

    let mut rng = StdRng::from_entropy();
    let mut tp = PrintThroughput::new("nodes");
    let stop = |tree: &Tree<_>| {
        tp.update_total(tree.root_visits());
        tree.root_visits() >= visits
    };

    if real {
        let graph = optimize_graph(&load_graph_from_onnx_path(path), Default::default());
        let mut network = CudaNetwork::new(mapper, &graph, settings.batch_size, Device::new(0));
        settings.build_tree(&board, &mut network, &mut rng, stop)
    } else {
        settings.build_tree(&board, &mut DummyNetwork, &mut rng, stop)
    }
}
