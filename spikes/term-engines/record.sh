#!/bin/sh
# Records the programs ADR 0120's comparison replays: each runs in a PTY under macOS
# `script`, at 120x40 with TERM=xterm-256color, with keys typed on a timer. Output goes to
# recordings/ (not committed). Needs vim, less, htop, lazygit, vttest and claude.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out="$here/recordings"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"

# rec NAME KEYS-SCRIPT COMMAND...: KEYS-SCRIPT is shell run to produce the typed keys.
rec() {
    name=$1 keys=$2
    shift 2
    sh -c "$keys" | TERM=xterm-256color COLORTERM=truecolor script -q "$out/$name.bin" \
        sh -c 'stty rows 40 cols 120; exec "$@"' sh "$@" >/dev/null 2>&1 || true
    echo "$name: $(wc -c < "$out/$name.bin") bytes"
}

# A text file with Japanese, Chinese, Korean, emoji, combining marks and ZWJ sequences.
cat > "$work/wide.txt" <<'TXT'
日本語のテキスト: 吾輩は猫である。名前はまだ無い。どこで生れたかとんと見当がつかぬ。
中文: 我能吞下玻璃而不伤身体。 한국어: 나는 유리를 먹을 수 있어요.
Combining: é → é, ä → ä, Zalgo: Z̤͔ͧ̑̓ä͖̭̈̇lͮ̒ͫǧ̗͚̚o̙̔ͮ̇͐̇
Emoji: 👍 👍🏽 👨‍👩‍👧‍👦 🇯🇵 ❤️ ☺︎ ✔️
Fullwidth: ＡＢＣ１２３ Halfwidth kana: ｱｲｳｴｵ
Box: ┌──┬──┐ │日│本│ └──┴──┘
TXT
for i in $(seq 1 200); do echo "$i 吾輩は猫である。名前はまだ無い。 The quick brown fox 👍 é $i" >> "$work/wide.txt"; done

adr="$here/../../docs/dev/adr/0120-the-terminal.md"

rec vim 'sleep 1; printf G; sleep .3; printf gg; sleep .3; printf "/Engine\r"; sleep .3; printf "n"; sleep .3;
    printf ":vsplit\r"; sleep .3; printf "20j"; sleep .3; printf "\027w"; sleep .3; printf "ihello 日本語 \033"; sleep .3;
    printf ":set nu\r"; sleep .3; printf ":qa!\r"; sleep .5' \
    vim -u NONE -N -c "syntax on" -c "set laststatus=2 ruler" "$adr"

rec vim-wide 'sleep 1; printf "10j"; sleep .3; printf "50%%"; sleep .3; printf "\006\006"; sleep .3;
    printf "ggA 追加 👍\033"; sleep .3; printf ":set nowrap\r"; sleep .3; printf "40l"; sleep .3; printf ":qa!\r"; sleep .5' \
    vim -u NONE -N -c "set encoding=utf-8 laststatus=2" "$work/wide.txt"

rec less 'sleep 1; printf " "; sleep .3; printf " "; sleep .3; printf "/terminal\r"; sleep .3; printf "n"; sleep .3;
    printf "G"; sleep .3; printf "g"; sleep .3; printf "q"; sleep .3' \
    less -R "$adr"

rec less-wide 'sleep 1; printf " "; sleep .3; printf "/猫\r"; sleep .3; printf "n"; sleep .3; printf "q"; sleep .3' \
    less -R "$work/wide.txt"

sleep 600 & sleeper=$!
rec htop 'sleep 2; printf "\033[B"; sleep 1; printf t; sleep 1; printf t; sleep 1; printf "\033OQ"; sleep 1; printf "\033"; sleep 1; printf q; sleep .5' \
    htop -d 5 -p "$sleeper,$$"
kill "$sleeper" 2>/dev/null || true

repo="$work/repo"
mkdir -p "$repo" "$work/lazygit"
printf 'update:\n  method: never\ndisableStartupPopups: true\n' > "$work/lazygit/config.yml"
(
    cd "$repo"
    git init -q
    git config user.name "Test User"
    git config user.email test@example.com
    for i in 1 2 3 4 5; do echo "line $i 日本語" >> file.txt; git add file.txt; git commit -qm "Commit $i: change 変更"; done
    echo "modified" >> file.txt
    echo "new" > new.txt
)
(cd "$repo" && rec lazygit 'sleep 3; printf j; sleep .5; printf " "; sleep .5; printf "\t"; sleep .5; printf "4"; sleep .5; printf j; sleep .5;
    printf "\r"; sleep .5; printf "\033"; sleep .5; printf "?"; sleep .5; printf "\033"; sleep .5; printf q; sleep 1' \
    env CONFIG_DIR="$work/lazygit" XDG_CONFIG_HOME="$work" lazygit)

# vttest: menu 1 (cursor movement) and 2 (screen features), each screen ended by RETURN.
rec vttest 'sleep 1; printf "1\r"; for i in 1 2 3 4 5 6; do sleep .5; printf "\r"; done;
    sleep .5; printf "2\r"; for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do sleep .5; printf "\r"; done;
    sleep .5; printf "0\r"; sleep .5' \
    vttest

# Claude Code in this directory: the trust prompt (answered yes), the start screen,
# typed text with wide characters that is never sent, `?`, /help and /exit. No prompt is
# sent to a model.
(cd "$here" && rec claude-code 'sleep 4; printf "\033[B"; sleep 1; printf "\r"; sleep 5; printf "hello 日本語のテキスト 👍 world"; sleep 1; printf "\025"; sleep 1; printf "?"; sleep 1; printf "\033"; sleep 1;
    printf "/help"; sleep 1; printf "\r"; sleep 3; printf "\033"; sleep 1; printf "/exit"; sleep 1; printf "\r"; sleep 2' \
    claude)
