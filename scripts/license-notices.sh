#!/usr/bin/env bash
# Writes the license notices that ship next to clankshift.exe into the given directory:
#   THIRD-PARTY-LICENSES.txt  crates and fonts (cargo-about, see about.toml)
#   RUST-LICENSES.html        Rust standard library, from the toolchain building the release
# Fails if any expected notice is missing. Used by CI and the release workflow.
set -euo pipefail
out=${1:?usage: license-notices.sh <output dir>}
mkdir -p "$out"

crates="$out/THIRD-PARTY-LICENSES.txt"
cargo about generate --locked about.hbs -o "$crates"
# cargo-about only warns if a clarified font license file changes, silently dropping it.
for text in 'SIL OPEN FONT LICENSE Version 1.1' 'UBUNTU FONT LICENCE Version 1.0' 'BITSTREAM VERA LICENSE' 'John Slegers'; do
  grep -qF "$text" "$crates" || { echo "missing font license: $text"; exit 1; }
done

# Rust's inventory names in-tree licenses by SPDX id only; their texts are in licenses/.
doc="$(rustc --print sysroot)/share/doc/rust"
inventory="$doc/COPYRIGHT-library.html"
grep -qF 'Copyright notices for The Rust Standard Library' "$inventory"
ids=$(sed '/id="out-of-tree-dependencies"/q' "$inventory" | grep -o '<b>License:</b> [^<]*' |
  sed 's/<b>License:<\/b>//; s/[()]/ /g' | tr ' ' '\n' | grep -vxE 'AND|OR|WITH|' | sort -u)
[ -n "$ids" ]
{
  sed '/<\/body>/,$d' "$inventory"
  echo "<h2 id=\"license-texts\">License texts</h2>"
  echo "<p>Built with $(rustc -V).</p>"
  for id in $ids; do
    echo "<h3>$id</h3><pre>"
    sed 's/&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g' "$doc/licenses/$id.txt"
    echo "</pre>"
  done
  echo "</body>"
  echo "</html>"
} > "$out/RUST-LICENSES.html"
echo "license notices written to $out (Rust in-tree licenses: $(echo $ids))"
