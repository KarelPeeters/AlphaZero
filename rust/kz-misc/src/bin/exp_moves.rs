use board_game::ai::solver::{solve, SolverValue};
use board_game::board::Board;
use board_game::games::chess::{chess_game_to_pgn, ChessBoard};
use kz_core::mapping::chess::ChessStdMapper;
use kz_core::mapping::PolicyMapper;
use rand::thread_rng;

fn main() {
    let mv_indices = vec![
        1478, 226, 202, 70, 1558, 225, 153, 1479, 1597, 596, 52, 1459, 1709, 539, 315, 1522, 65, 1568, 134, 225, 1784,
        1588, 87, 269, 339, 548, 305, 293, 134, 1683, 66, 87, 91, 66, 237, 0, 49, 505, 87, 711, 65, 93, 720, 511, 193,
        281, 305, 87, 505, 66, 44, 177, 66, 339, 214, 720, 351, 165, 203, 87, 608, 1478, 114, 1557, 71, 1519, 256, 71,
        314, 240, 480, 214, 638, 72, 430, 1274, 572, 259, 597, 92, 209, 258, 805, 710, 132, 66, 1250, 51, 765, 1617,
        938, 456, 624, 893, 157, 23, 516, 335, 293, 253, 161, 74, 915, 88, 1458, 434, 595, 882, 88, 1656, 114, 679,
        316, 800, 4, 312, 111, 109, 527, 1746, 755, 290, 719, 1784, 155, 1720, 946, 1651, 689, 1561, 502, 335, 460,
        649, 478, 88, 506, 313, 882, 110, 711, 1669, 177, 337, 680, 1706, 339, 519, 1164, 1740, 997, 242, 503, 131,
        666, 286, 550, 1781, 757, 109, 582, 221, 526, 335, 700, 359, 915, 551, 1096, 1736, 492, 309, 182, 931, 823, 91,
        1425, 982, 1817, 232, 1314, 1157, 905, 599, 94, 1107, 712, 246, 1231, 430, 516, 1649, 414, 1749, 1020, 1690,
        587, 590, 152, 616, 422, 1728, 1252, 586, 258, 405, 125, 843, 72, 565, 828, 597, 690, 815, 268, 1632, 1413,
        973, 1451, 1034, 336, 758, 420, 734, 518, 1729, 335, 1692, 312, 540, 214, 1768, 70, 1870, 291, 352, 1344, 372,
        212, 1780, 1294, 1713, 511, 193, 30, 1673, 445, 222, 237, 1676, 546, 1608, 88, 371, 112, 169, 669, 29, 258,
        1580, 229, 1538, 65, 1464, 591, 1538, 51, 189, 235, 1676, 51, 1579, 636, 1649, 237, 1748, 874, 7, 88, 1663,
        112, 1725, 259, 1664, 1333, 1750, 534, 169, 1340, 29, 1020, 190, 584, 1723, 268, 1790, 432, 1719, 648, 1621,
        93, 1560, 291, 22, 818, 44, 1149, 72, 1297, 1653, 820, 268, 1222, 420, 178, 222, 381, 383, 554, 1614, 451,
        1511, 425, 1600, 1197, 1537, 1375, 587, 406, 1645, 552, 432, 595, 1606, 758, 1503, 736, 639, 757, 1531, 596,
        445, 784, 235, 806, 1466, 622, 50, 805, 222, 586, 381, 407, 1510, 616, 1569, 596, 1510, 780, 549, 597, 744,
        810, 1571, 783, 1669, 765, 959, 930, 1706, 959, 1739, 1127, 1118, 1300, 1766, 1119, 929, 960, 758, 1150, 735,
        1320, 571, 1118, 1680, 937, 735, 1128, 1575, 1324, 563, 1345, 400, 1324, 426, 1342, 1683, 1172, 1744,
    ];

    let mapper = ChessStdMapper;
    let start = ChessBoard::default();

    let mut board = start.clone();
    let mut moves = vec![];
    let mut rng = thread_rng();

    for mv_index in mv_indices {
        // check for mate-in-one
        let solution = solve(&board, 1, &mut rng);
        if let SolverValue::WinIn(n) = solution.value {
            println!("Mate in {} for {:?}:", n, board.inner().side_to_move());
            println!("{}", board.inner().to_string());
        }

        let mv = mapper.index_to_move(&board, mv_index).unwrap();
        moves.push(mv);
        board.play(mv).unwrap();
    }

    println!("{}", chess_game_to_pgn("muZero", "muZero", &start, &moves));
}
