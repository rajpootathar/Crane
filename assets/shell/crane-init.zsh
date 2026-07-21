# Crane shell integration (zsh). Emits OSC 633 (VS Code convention) so Crane
# can record per-directory, session-scoped command history. Safe to source in
# any zsh; a non-Crane terminal simply ignores the escape sequences.

# Guard against double-sourcing. `:-` matters: the user's rc may have run
# `setopt nounset` before we load, which would make a bare reference to an
# unset parameter a hard error and abort this file on its first line — no
# integration at all, and an error printed into their shell.
if [[ -n "${CRANE_SHELL_INTEGRATION:-}" ]]; then
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
  # Report the line-editor keymap so Crane can disable its emacs-only (^E^U)
  # up/down history interception in vi mode and let zsh's own vi history run.
  # Emitted every prompt so a mid-session `bindkey -v` / `set -o vi` is caught.
  # `bindkey -lL main` prints the keymap `main` is linked to (`viins` for vi via
  # either `bindkey -v` OR `set -o vi`, `emacs` otherwise) — `[[ -o vi ]]` alone
  # would miss `bindkey -v`. Best-effort: if `bindkey` misbehaves the match just
  # falls through to emacs and never aborts the prompt.
  if [[ "$(bindkey -lL main)" == *vi* ]]; then
    __crane_osc "P;Keymap=vi"
  else
    __crane_osc "P;Keymap=emacs"
  fi
  __crane_osc "A"   # prompt start
  __crane_osc "B"   # command start
}

__crane_preexec() {
  __crane_osc "E;$(__crane_escape "$1")"
  __crane_osc "C"   # pre-execution
  __crane_executing=1
}

# Plain append. zsh restores `lastval` around every element of a hook array —
# `man zshmisc`, "Hook Functions": each function runs "in the same context and
# with the same arguments and same initial value of $? as the basic function".
# So an earlier precmd hook shelling out (a git prompt running `git status`)
# cannot clobber $? for ours, and there is no reason to force ourselves to the
# front of precmd_functions. Running last is also the better order: hooks that
# print (direnv, nvm, conda notices) then emit their output BEFORE our
# 633;A/633;B markers rather than inside the marked prompt region.
autoload -Uz add-zsh-hook
add-zsh-hook precmd __crane_precmd
add-zsh-hook preexec __crane_preexec
