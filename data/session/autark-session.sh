# Sourced by login shells (install as /etc/profile.d/autark-session.sh). Wayfire reads the cursor theme and size from its
# environment when it starts, not from wayfire.ini, so a session started from a login shell needs these first.
: "${XCURSOR_THEME:=Autark}"
: "${XCURSOR_SIZE:=24}"
export XCURSOR_THEME XCURSOR_SIZE
