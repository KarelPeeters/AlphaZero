use std::cmp::{max, min};
use std::collections::HashSet;

use board_game::board::Board;
use board_game::games::chess::ChessBoard;
use board_game::wdl::{Flip, OutcomeWDL};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use itertools::Itertools;
use tui::backend::CrosstermBackend;
use tui::buffer::Buffer;
use tui::layout::{Margin, Rect};
use tui::style::{Modifier, Style};
use tui::Terminal;
use tui::widgets::Widget;

use alpha_zero::network::dummy::DummyNetwork;
use alpha_zero::oracle::DummyOracle;
use alpha_zero::util::display_option_empty;
use alpha_zero::zero::node::{Uct, UctWeights, ZeroValues};
use alpha_zero::zero::step::FpuMode;
use alpha_zero::zero::tree::Tree;
use alpha_zero::zero::wrapper::ZeroSettings;

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
        tree: build_tree(),
        expanded_nodes: HashSet::default(),
        selected_node: 0,
        view_offset: 0,
    };

    // println!("{}", state.tree.display(2, true, 200, false));
    // return Ok(());

    state.expanded_nodes.insert(0);
    state.expanded_nodes.insert(1);

    // setup terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    loop {
        let mut prev_area = None;

        terminal.draw(|f| {
            let area = f.size().inner(&Margin { horizontal: 2, vertical: 2 });

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
    execute!(terminal.backend_mut(),LeaveAlternateScreen,DisableMouseCapture)?;
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
            for c in self.tree[curr].children.iter().flat_map(|r| r.iter()) {
                self.append_nodes(c, depth + 1, result);
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
            selected as i32 - area.height as i32 + margin as i32 + 1,
            selected.saturating_sub(margin) as i32,
        );

        assert!(offset >= 0, "offset={}", offset);
        self.view_offset = offset as usize;
    }

    fn selected_index(&self) -> usize {
        self.prev_nodes.iter().position(|n| n.node == self.selected_node).unwrap()
    }

    fn handle_event(&mut self, area: Rect, event: Event) {
        match event {
            Event::Key(key) => {
                match key.code {
                    KeyCode::Up => {
                        let index = self.selected_index();
                        if index != 0 {
                            self.selected_node = self.prev_nodes[index - 1].node;
                        }
                    }
                    KeyCode::Down => {
                        self.selected_node = self.prev_nodes.get(self.selected_index() + 1)
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
                }
            }
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
        let mut col_sizes = vec![0; 1 + COLUMN_NAMES.len()];
        col_sizes[0] = 20;

        for (i, (n1, n2, _)) in COLUMN_NAMES.iter().enumerate() {
            col_sizes[i] = max(col_sizes[i], max(n1.len(), n2.len()) as u16);
        }

        for &RenderNode { node, depth } in &self.prev_nodes {
            for (i, v) in self.column_values(node, depth).iter().enumerate() {
                col_sizes[i] = max(col_sizes[i], v.len() as u16);
            }
        }

        let col_starts = col_sizes.iter().scan(area.x, |curr, &size| {
            *curr += size + COL_SPACING;
            Some(*curr - size - COL_SPACING)
        }).collect_vec();

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

        let terminal = match node.outcome() {
            Err(_) => '?',
            Ok(None) => '.',
            Ok(Some(OutcomeWDL::Win)) => 'W',
            Ok(Some(OutcomeWDL::Draw)) => 'D',
            Ok(Some(OutcomeWDL::Loss)) => 'L',
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
            let zero = node.values();
            let net = node.net_values.unwrap_or(ZeroValues::nan());
            let uct = if let Some(parent) = node.parent {
                let parent = &self.tree[parent];
                node.uct(parent.total_visits(), parent.values().flip(), false)
            } else {
                Uct::nan()
            };

            let values = [
                zero.wdl.win, zero.wdl.draw, zero.wdl.loss, zero.moves_left,
                net.wdl.win, net.wdl.draw, net.wdl.loss, net.moves_left,
                uct.v, uct.u, uct.m,
            ];
            result.extend(values.iter().map(|v| if v.is_nan() { "".to_owned() } else { format!("{:.3}", v) }));
        }

        assert_eq!(result.len(), COLUMN_NAMES.len());
        result
    }
}

const COLUMN_NAMES: &[(&str, &str, bool)] = &[
    ("Node", "", false), ("Move", "", false), ("T", "", false), ("Visits", "", true),
    ("Zero", "W", true), ("Zero", "D", true), ("Zero", "L", true), ("Zero", "M", true),
    ("Net", "W", true), ("Net", "D", true), ("Net", "L", true), ("Net", "M", true),
    ("Uct", "V", true), ("Uct", "U", true), ("Uct", "M", true),
];

impl<B: Board> Widget for &State<B> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (col_sizes, col_starts) = self.compute_col_starts(area);

        for (i, &(n1, n2, _)) in COLUMN_NAMES.iter().enumerate() {
            if i == 0 || COLUMN_NAMES[i - 1].0 != n1 {
                buf.set_string(col_starts[i], area.y, n1, Style::default());
            }
            buf.set_string(col_starts[i], area.y + 1, n2, Style::default());
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
                    let just_right = COLUMN_NAMES[i].2;

                    let x = if just_right {
                        col_starts[i] + (col_sizes[i] - v.len() as u16)
                    } else {
                        col_starts[i]
                    };

                    buf.set_string(x, full_y, v, Style::default());
                }
            }
        }
    }
}

fn build_tree() -> Tree<ChessBoard> {
    let settings = ZeroSettings::new(128, UctWeights::default(), false, FpuMode::Parent);
    let visits = 1_000;

    // let path = "C:/Documents/Programming/STTT/AlphaZero/data/networks/chess_real_1859.onnx";
    // let graph = optimize_graph(&load_graph_from_onnx_path(path), Default::default());
    // let mut network = CudnnNetwork::new(ChessStdMapper, graph, settings.batch_size, Device::new(0));
    let mut network = DummyNetwork;

    let board = ChessBoard::default();
    let tree = settings.build_tree(&board, &mut network, &DummyOracle, |tree| tree.root_visits() >= visits);

    tree
}