from harness import context, read_input

PREAMBLE = """dusk-dev is loaded.
- /dusk-dev:activate loads what there is to know about dusk - the dusk-developer orientation map and working agreements, building and measuring, driving a node by hand. /what reports where things stand; /honest-to-god before reporting a finding, a risk or a limitation.
- Hooks enforce, in every session, as the action happens: nothing personal written anywhere (no name, no email address, no home directory, no account handle); no comment lines written by me; commit subjects with no feat:-style prefix, no Say, no (#N), no watermark trailer; git commit in the foreground with timeout 600000; no polling loops; no cp of a target directory; no bare git stash; no push to master; merging always asks."""


def main():
    read_input()
    context(PREAMBLE)


if __name__ == "__main__":
    main()
