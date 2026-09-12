from harness import ask, read_input


def main():
    read_input()
    ask("Merging is the one gate that may not be skipped: did the user approve this merge, explicitly, for this pull request?")


if __name__ == "__main__":
    main()
