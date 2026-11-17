# Source before running cargo locally: `source ./env.sh`
_pc_root="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
if [ -d /opt/homebrew/opt/rustup/bin ]; then export PATH="/opt/homebrew/opt/rustup/bin:$PATH"; fi
if [ -d "$_pc_root/.cargo-home" ]; then export CARGO_HOME="$_pc_root/.cargo-home"; fi
unset _pc_root
