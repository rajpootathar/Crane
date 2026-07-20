# Crane shell integration (bash). Emits OSC 633 via PROMPT_COMMAND + a DEBUG
# trap. Sourced via --rcfile after the user's ~/.bashrc.

# Guard against double-sourcing.
if [[ -n "$CRANE_SHELL_INTEGRATION" ]]; then
  return 0
fi
CRANE_SHELL_INTEGRATION=1

[[ -f "$HOME/.bashrc" ]] && source "$HOME/.bashrc"

__crane_osc() { printf '\e]633;%s\a' "$1"; }
__crane_escape() {
  local s=${1//\\/\\x5c}; s=${s//;/\\x3b}; s=${s//$'\n'/\\x0a}; printf '%s' "$s"
}

# Predeclare so a `set -u` picked up from the user's own bashrc can't turn a
# reference to these (before their first assignment) into a hard error.
__crane_executing=""
__crane_in_precmd=""

# Runs once per prompt cycle: reports the just-finished command's exit code,
# then the new cwd and prompt-start/command-start markers. Must be the very
# first thing __crane_prompt_command does so $? still reflects the command
# that just finished, not the `__crane_in_precmd=1` assignment that follows.
__crane_precmd() {
  local exit=$?
  if [[ -n "$__crane_executing" ]]; then __crane_osc "D;$exit"; __crane_executing=""; fi
  __crane_osc "P;Cwd=$PWD"; __crane_osc "A"; __crane_osc "B"
}

# DEBUG fires before every simple command bash runs — including each one
# inside PROMPT_COMMAND itself (ours, and the user's pre-existing one, which
# we `eval` from inside __crane_prompt_command below) and every step of a
# compound command the user types. Naively excluding only the literal
# command name "__crane_prompt" (as opposed to wrapping the whole prompt
# cycle) would misfire on the user's own PROMPT_COMMAND — e.g. a prompt
# theme's hook function — recording it as though it were the next typed
# command and then suppressing the real one. __crane_in_precmd brackets the
# whole wrapped run so none of it is ever mistaken for a typed command;
# __crane_executing then makes sure only the FIRST simple command seen after
# a prompt (the one the user actually typed) gets recorded. The name check
# covers the single DEBUG firing for invoking the wrapper itself, which
# happens just before __crane_in_precmd is set.
__crane_debug() {
  case "$BASH_COMMAND" in
    __crane_prompt_command) return ;;
  esac
  [[ -n "$__crane_in_precmd" ]] && return
  if [[ -z "$__crane_executing" ]]; then
    __crane_osc "E;$(__crane_escape "$BASH_COMMAND")"; __crane_osc "C"; __crane_executing=1
  fi
}

# Wrap (rather than string-concatenate) the pre-existing PROMPT_COMMAND so
# the DEBUG trap can tell "still running PROMPT_COMMAND" apart from "the
# next real command", no matter how many statements the user's own
# PROMPT_COMMAND contains.
__crane_user_prompt_command="$PROMPT_COMMAND"
__crane_prompt_command() {
  __crane_precmd
  __crane_in_precmd=1
  if [[ -n "$__crane_user_prompt_command" ]]; then
    eval "$__crane_user_prompt_command"
  fi
  __crane_in_precmd=""
}
PROMPT_COMMAND="__crane_prompt_command"
trap '__crane_debug' DEBUG
