from lib.dataset import GameDataFile
from lib.games import Game


def main():
    game = Game.find("chess")
    path = "../../data/new_loop/test/selfplay/games_34.bin.gz"
    file = GameDataFile(game, path)

    train, test = file.split_dataset(0.1)

    print(len(train))
    print(len(test))

if __name__ == '__main__':
    main()