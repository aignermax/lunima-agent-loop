#!/bin/sh
# Installs Lunima Team from this tarball.
#   ./install.sh               → ~/.local (no root needed), user unit in ~/.config/systemd/user
#   sudo ./install.sh --system → /usr/local, user unit in /etc/systemd/user (for every user)
#   ./install.sh --uninstall [--system]
# Config, state and logs (~/.local/share/lunima-team) are never touched.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
mode=user
action=install
for arg in "$@"; do
    case "$arg" in
        --system) mode=system ;;
        --uninstall) action=uninstall ;;
        -h|--help) sed -n '2,6p' "$0"; exit 0 ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

if [ "$mode" = system ]; then
    prefix=/usr/local
    unit_dir=/etc/systemd/user
else
    prefix="$HOME/.local"
    unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
fi
bin_dir="$prefix/bin"
app_dir="$prefix/share/applications"
icon_dir="$prefix/share/icons/hicolor/256x256/apps"
unit="$unit_dir/lunima-team-agent.service"

reload_units() {
    if [ "$mode" = user ] && command -v systemctl >/dev/null 2>&1; then
        systemctl --user daemon-reload 2>/dev/null || true
    fi
}

if [ "$action" = uninstall ]; then
    if [ "$mode" = user ] && command -v systemctl >/dev/null 2>&1; then
        systemctl --user disable --now lunima-team-agent 2>/dev/null || true
    fi
    rm -f "$bin_dir/lunima-team-agent" "$bin_dir/lunima-po" "$unit" \
        "$app_dir/lunima-team-control-center.desktop" "$icon_dir/lunima-team-control-center.png"
    reload_units
    echo "Lunima Team removed (data in ~/.local/share/lunima-team kept)."
    exit 0
fi

mkdir -p "$bin_dir" "$app_dir" "$icon_dir" "$unit_dir"
install -m 755 "$here/bin/lunima-team-agent" "$bin_dir/lunima-team-agent"
install -m 755 "$here/bin/lunima-po" "$bin_dir/lunima-po"
install -m 644 "$here/share/icons/lunima-team-control-center.png" "$icon_dir/lunima-team-control-center.png"
sed "s|^Exec=.*|Exec=$bin_dir/lunima-po|" "$here/share/applications/lunima-team-control-center.desktop" \
    > "$app_dir/lunima-team-control-center.desktop"
sed "s|^ExecStart=.*|ExecStart=$bin_dir/lunima-team-agent run|" "$here/share/systemd/user/lunima-team-agent.service" > "$unit"
chmod 644 "$app_dir/lunima-team-control-center.desktop" "$unit"
reload_units

cat <<EOF
Lunima Team installed to $prefix.

Next steps:
  1. Prerequisites: git, gh (gh auth login), Claude Code CLI (claude, logged in with your subscription)
  2. lunima-team-agent init                              # writes ~/.local/share/lunima-team/team.toml
  3. systemctl --user enable --now lunima-team-agent     # runs the agent in the background
  4. lunima-po                                           # Control Center (status, pause, settings)
EOF
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "Note: $bin_dir is not on your PATH yet." ;;
esac
