# Sourced before every command inyon runs on Windows (busybox-w32 sh).
# Windows: the hub speaks POSIX sh to every machine, so commands run in busybox-w32's sh, and
# `tmux` is psmux. psmux numbers panes and windows per session (every session has a %1), so
# the hub sees them as %<session>.<pane> and @<session>.<window>; this prelude turns those back
# into psmux targets ($14:.%1), prints ids in the same form, and fills in what psmux and busybox
# lack (list-clients -F, ps with command lines).
tmux() {
  if [ "$1" = list-clients ]; then
    # psmux ignores -F here; the hub only asks for "<session id> <control mode>"
    for _s in $(command tmux list-sessions -F '#{session_id}' 2>/dev/null); do
      command tmux list-clients -t "$_s" 2>/dev/null | while IFS= read -r _l; do
        case $_l in *'(control mode)'*) echo "$_s 1" ;; ?*) echo "$_s 0" ;; esac
      done
    done
    return 0
  fi
  # ids only right after -t/-s; nothing after -l or -- (that's text being typed)
  _n=$# _fix= _prev= _raw=
  while [ "$_n" -gt 0 ]; do
    _x=$1; _o=$1; shift; _n=$((_n - 1))
    if [ -z "$_raw" ]; then
      case $_prev in -t | -s)
        case $_x in
          %[0-9]*.[0-9]*) _s=${_x#%}; _x="\$${_s%%.*}:.%${_s#*.}" ;;
          @[0-9]*.[0-9]*) _s=${_x#@}; _x="\$${_s%%.*}:@${_s#*.}" ;;
        esac ;;
      esac
      case $_x in
        -- | -l) _raw=1 ;;
        *'#{pane_id}'* | *'#{window_id}'*)
          _fix=1
          # psmux doubles a literal % in a format (strftime-style), so ask for $14:%1 and rewrite it below
          _x=$(printf '%s' "$_x" | sed 's/#{pane_id}/#{session_id}:#{pane_id}/g; s/#{window_id}/#{session_id}:#{window_id}/g') ;;
      esac
    fi
    _prev=$_o
    set -- "$@" "$_x"
  done
  [ -z "$_fix" ] && { command tmux "$@"; return; }
  _out=$(command tmux "$@"); _rc=$?
  [ -n "$_out" ] && printf '%s\n' "$_out" | sed 's/\$\([0-9][0-9]*\):%\([0-9][0-9]*\)/%\1.\2/g; s/\$\([0-9][0-9]*\):@\([0-9][0-9]*\)/@\1.\2/g'
  return $_rc
}
ps() { cat "$INYON_PS" 2>/dev/null; }
pgrep() { return 1; }
