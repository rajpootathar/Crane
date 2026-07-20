# Crane shell integration (zsh). Emits OSC 633 (VS Code convention) so Crane
# can record per-directory, session-scoped command history. Safe to source in
# any zsh; a non-Crane terminal simply ignores the escape sequences.

# Guard against double-sourcing.
if [[ -n "$CRANE_SHELL_INTEGRATION" ]]; then
  return 0
fi
CRANE_SHELL_INTEGRATION=1

# Predeclare so `setopt nounset` in the user's own rc can't turn a reference
# to these before their first assignment into a hard error.
typeset -g __crane_executing=""

__crane_osc() { printf '\e]633;%s\a' "$1"; }

# Escape a command line for OSC 633;E: backslash, semicolon, newline.
__crane_escape() {
  local s=${1//\\/\\x5c}
  s=${s//;/\\x3b}
  s=${s//$'\n'/\\x0a}
  printf '%s' "$s"
}

__crane_precmd() {
  local exit=$?
  # Report the just-finished command's exit code (skip on the very first prompt).
  if [[ -n "$__crane_executing" ]]; then
    __crane_osc "D;$exit"
    __crane_executing=""
  fi
  __crane_osc "P;Cwd=$PWD"
  __crane_osc "A"   # prompt start
  __crane_osc "B"   # command start
}

__crane_preexec() {
  __crane_osc "E;$(__crane_escape "$1")"
  __crane_osc "C"   # pre-execution
  __crane_executing=1
}

autoload -Uz add-zsh-hook
add-zsh-hook precmd __crane_precmd
add-zsh-hook preexec __crane_preexec

# zsh runs precmd_functions in array order and $? is a single global shared
# by all of them. Since our .zshrc shim sources the user's own rc (and any
# prompt framework it installs) BEFORE loading us, add-zsh-hook would append
# us last — meaning an earlier hook's own internal commands (e.g. a git
# prompt shelling out to `git status`) could overwrite $? before we ever
# read it, reporting the wrong exit code. Move ourselves to the front so we
# capture $? before any other precmd hook gets a chance to touch it.
precmd_functions=(__crane_precmd ${precmd_functions:#__crane_precmd})
