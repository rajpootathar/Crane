# Crane shell integration (bash). Emits OSC 633 via PROMPT_COMMAND + a DEBUG
# trap. Sourced via --rcfile after the user's ~/.bashrc.

# Guard against double-sourcing. `:-` matters: this file can be sourced from a
# shell that already has `set -u`, which would make a bare reference to an unset
# parameter a hard error and abort on the first line.
if [[ -n "${CRANE_SHELL_INTEGRATION:-}" ]]; then
  return 0
fi
CRANE_SHELL_INTEGRATION=1

[[ -f "${HOME:-}/.bashrc" ]] && source "$HOME/.bashrc"

__crane_osc() { printf '\e]633;%s\a' "$1"; }
__crane_escape() {
  local s=${1//\\/\\x5c}; s=${s//;/\\x3b}; s=${s//$'\n'/\\x0a}; printf '%s' "$s"
}

# Predeclare so a `set -u` picked up from the user's own bashrc can't turn a
# reference to these (before their first assignment) into a hard error.
__crane_executing=""
__crane_in_precmd=""
# History bookkeeping, see __crane_read_history / __crane_command_line below.
__crane_hist_num=""
__crane_hist_cur=""
__crane_line=""

# Reports the just-finished command's exit code, then the new cwd and the
# prompt-start / command-start markers. Takes the exit code as an argument
# because __crane_prompt_command has to latch $? before anything else runs.
__crane_precmd() {
  local exit="$1"
  if [[ -n "$__crane_executing" ]]; then __crane_osc "D;$exit"; __crane_executing=""; fi
  # Re-baseline the history counter while sitting at a prompt: whatever
  # `history 1` reports now is the PREVIOUS command, so anything the next
  # DEBUG trap sees under that same number was never recorded. Doing it here
  # rather than once at load time also covers the first command of a session,
  # which would otherwise be compared against an empty baseline and wrongly
  # accept the last line restored from HISTFILE.
  __crane_read_history && __crane_hist_num="$__crane_hist_cur"
  __crane_osc "P;Cwd=$PWD"; __crane_osc "A"; __crane_osc "B"
}

# $BASH_COMMAND is only the current SIMPLE command, so `foo && bar` records as
# `foo` and `ls | wc -l` as `ls` — a large share of real command lines, and
# asymmetric with zsh's preexec, which gets the full raw line as $1. The full
# line is what bash just appended to history, and `history 1` returns it
# verbatim, embedded newlines and all.
#
# `fc -lnr -0` reads as the tidier spelling but is off by one from inside a
# DEBUG trap: bash drops the newest history entry so an `fc` invocation never
# lists itself, so from here it yields the PREVIOUS command. Verified on bash
# 3.2 (the system bash on macOS) — during `true && false` it returned the
# preceding `ls | wc -l`, while `history 1` returned `true && false`.
#
# History is only enabled for interactive shells; anywhere else this comes back
# empty and the caller falls back to $BASH_COMMAND.
#
# Splits `history 1` into its entry number (__crane_hist_cur) and its text
# (__crane_line). Sets globals instead of printing because the staleness check
# in __crane_command_line has to carry state across calls, and a command
# substitution would run the whole thing in a subshell and discard it.
__crane_read_history() {
  __crane_hist_cur=""
  __crane_line=""
  local h
  h=$(HISTTIMEFORMAT= LC_ALL=C builtin history 1 2>/dev/null) || return 1
  [[ -n "$h" ]] || return 1
  # "  123  the command" / "  123* the command" -> "the command". Parameter
  # expansion rather than sed so it costs no extra fork, and unlike a regex it
  # keeps every line of a multi-line entry ('.' would not match the newlines).
  h=${h#"${h%%[![:space:]]*}"}   # leading blanks
  __crane_hist_cur=${h%%[![:digit:]]*}
  h=${h#"$__crane_hist_cur"}     # history number
  h=${h#\*}                      # "entry was modified" marker
  h=${h#"${h%%[![:space:]]*}"}   # blanks between number and command
  [[ -n "$__crane_hist_cur" && -n "$h" ]] || return 1
  __crane_line="$h"
}

# The line the user just typed, in __crane_line — but only when bash actually
# recorded it.
#
# HISTCONTROL / HISTIGNORE let bash decline: `HISTCONTROL=ignoreboth` is the
# stock default in Debian/Ubuntu's /etc/skel/.bashrc and drops any line with a
# leading space or one identical to its predecessor. `history 1` then still
# succeeds — it just returns the PRIOR entry, so the previous command gets
# reported as the current one, and a deliberately space-hidden line is
# attributed to whatever came before it.
#
# The entry number is the tell: a recorded line advances it past the baseline
# __crane_precmd took at the prompt, a dropped one leaves it untouched. `!=`
# rather than `-gt` so a `history -c` (which restarts numbering low) reads as a
# fresh entry instead of wedging the check forever.
__crane_command_line() {
  __crane_read_history || return 1
  [[ "$__crane_hist_cur" != "$__crane_hist_num" ]] || return 1
  __crane_hist_num="$__crane_hist_cur"
}

# DEBUG fires before every simple command bash runs — including each one inside
# PROMPT_COMMAND itself (ours, and the user's pre-existing one, which we `eval`
# from inside __crane_prompt_command below) and every step of a compound
# command the user types. Naively excluding only the literal command name
# "__crane_prompt_command" (as opposed to wrapping the whole prompt cycle)
# would misfire on the user's own PROMPT_COMMAND — e.g. a prompt theme's hook
# function — recording it as though it were the next typed command and then
# suppressing the real one. __crane_in_precmd brackets the whole wrapped run so
# none of it is ever mistaken for a typed command; the name check covers the
# single DEBUG firing for invoking the wrapper itself, which happens just
# before __crane_in_precmd is set.
#
# __crane_executing then collapses a compound line to one E/C pair, matching
# the single D that __crane_precmd emits. For a pipeline bash fires DEBUG once
# per element but does so in the PARENT shell, before forking — verified, so
# the latch really does stick and `ls | wc -l` still gets its D.
__crane_debug() {
  case "$BASH_COMMAND" in
    __crane_prompt_command) return ;;
  esac
  [[ -n "$__crane_in_precmd" ]] && return
  [[ -n "$__crane_executing" ]] && return
  # Called directly, never as `$(...)`: __crane_command_line carries the
  # history counter across invocations and a subshell would throw it away.
  local line
  if __crane_command_line; then line="$__crane_line"; else line="$BASH_COMMAND"; fi
  __crane_osc "E;$(__crane_escape "$line")"
  __crane_osc "C"
  __crane_executing=1
}

# bash 5.1 made PROMPT_COMMAND assignable as an ARRAY, and `PROMPT_COMMAND+=(…)`
# is now a common idiom (direnv, conda, …). Reading "$PROMPT_COMMAND" on an
# array yields element 0 only, and assigning a scalar over it replaces index 0
# while leaving 1..n in place — bash still runs them, but OUTSIDE our wrapper,
# where the DEBUG trap records them as if the user had typed them. So capture
# every element whatever the form, drop the variable entirely, then install the
# wrapper as the sole PROMPT_COMMAND.
__crane_user_prompt_commands=()
if [[ -n "${PROMPT_COMMAND+x}" ]]; then
  __crane_pc_decl=$(declare -p PROMPT_COMMAND 2>/dev/null)
  # `declare -p` prints "declare -<flags> NAME=…"; an 'a' among the flags means
  # array. Strip from the name onward so a stray 'a' in the value can't match.
  case "${__crane_pc_decl%% PROMPT_COMMAND*}" in
    declare\ -*a*) __crane_user_prompt_commands=("${PROMPT_COMMAND[@]}") ;;
    *)             __crane_user_prompt_commands=("$PROMPT_COMMAND") ;;
  esac
  unset __crane_pc_decl
fi
unset PROMPT_COMMAND 2>/dev/null

# Wrap (rather than string-concatenate) the pre-existing PROMPT_COMMAND so the
# DEBUG trap can tell "still running PROMPT_COMMAND" apart from "the next real
# command", no matter how many statements or elements it had.
__crane_prompt_command() {
  local __crane_exit=$?
  # Set the bracket before anything else so the guard holds even under
  # `set -o functrace`, where DEBUG does reach into shell functions.
  __crane_in_precmd=1
  __crane_precmd "$__crane_exit"
  local __crane_pc
  for __crane_pc in ${__crane_user_prompt_commands[@]+"${__crane_user_prompt_commands[@]}"}; do
    [[ -n "$__crane_pc" ]] && eval "$__crane_pc"
  done
  __crane_in_precmd=""
}
PROMPT_COMMAND="__crane_prompt_command"
trap '__crane_debug' DEBUG
