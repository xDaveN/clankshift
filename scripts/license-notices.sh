#!/usr/bin/env bash
# Writes LICENSES.txt, the license notices embedded in clankshift.exe (see build.rs), into the
# given directory: ClankShift's own licenses, crates and fonts (cargo-about, see about.toml), and
# the Rust standard library from the toolchain building the exe.
# Fails if any expected notice is missing. Used by CI and the release workflow.
set -euo pipefail
out=${1:?usage: license-notices.sh <output dir>}
mkdir -p "$out"
notices="$out/LICENSES.txt"

crates="$out/crates.txt"
cargo about generate --locked about.hbs -o "$crates"
# cargo-about only warns if a clarified font license file changes, silently dropping it.
for text in 'SIL OPEN FONT LICENSE Version 1.1' 'UBUNTU FONT LICENCE Version 1.0' 'BITSTREAM VERA LICENSE' 'John Slegers'; do
  grep -qF "$text" "$crates" || { echo "missing font license: $text"; exit 1; }
done

# Rust's inventory covers std and every crate its build uses. Keep the out-of-tree crates the
# target's std ships as rlibs (the only ones that can be linked in), and render it as text.
sysroot=$(rustc --print sysroot)
doc="$sysroot/share/doc/rust"
inventory="$doc/COPYRIGHT-library.html"
grep -qF 'Copyright notices for The Rust Standard Library' "$inventory"
libs=$(ls "$sysroot/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/lib" | sed -n 's/^lib\(.*\)-[0-9a-f]*\.rlib$/\1/p' | sort -u)
[ -n "$libs" ]
std="$out/std.txt"
LC_ALL=C awk -v libs="$libs" '
  BEGIN { n = split(libs, l, "\n"); for (i = 1; i <= n; i++) lib[l[i]] = 1; keep = 1 }
  function text(s) {
    gsub(/<[^>]*>/, "", s)
    gsub(/&#60;|&lt;/, "<", s); gsub(/&#62;|&gt;/, ">", s); gsub(/&#34;|&quot;/, "\"", s)
    gsub(/&#39;/, "\047", s); gsub(/&#38;|&amp;/, "\\&", s)
    return s
  }
  /<body>/ { body = 1; next }
  !body { next }
  /<h2>Table of Contents/ { toc = 1 }
  toc { if (/<\/ul>/) toc = 0; next }
  /<\/body>/ { exit }
  /<h3>/ {
    crate = text($0); sub(/^[ \t]*\360\237\223\246 /, "", crate); sub(/-[0-9]+\.[0-9]+\..*$/, "", crate)
    gsub(/-/, "_", crate); keep = (crate in lib); kept += keep
  }
  !keep { next }
  pre {
    if (/<\/pre>/) { pre = 0; sub(/<\/pre>.*/, ""); if ($0 ~ /[^ \t]/) print text($0); next }
    print text($0); next
  }
  /<pre>/ { pre = 1; sub(/.*<pre>/, ""); if ($0 ~ /[^ \t]/) print text($0); next }
  /<h[1-3]|File\/Directory:/ { print "" }
  { s = text($0); gsub(/^[ \t]+|[ \t]+$/, "", s); if (s != "") print s }
  END { if (!kept) { print "no out-of-tree std crate kept" > "/dev/stderr"; exit 1 } }
' "$inventory" > "$std"

# The inventory names in-tree licenses by SPDX id only; their texts are in licenses/.
ids=$(sed '/id="out-of-tree-dependencies"/q' "$inventory" | grep -o '<b>License:</b> [^<]*' |
  sed 's/<b>License:<\/b>//; s/[()]/ /g' | tr ' ' '\n' | grep -vxE 'AND|OR|WITH|' | sort -u)
[ -n "$ids" ]

rule='================================================================================'
{
  echo "ClankShift"
  echo
  echo "ClankShift is licensed under the MIT License or the Apache License 2.0, at your option."
  echo "Below are both licenses, then the notices of the third-party code built into this exe."
  for f in LICENSE-MIT LICENSE-APACHE; do
    echo; echo "$rule"; echo "ClankShift: $f"; echo; cat "$f"
  done
  echo; echo "$rule"; cat "$crates"
  echo; echo "$rule"; echo "Rust standard library (built with $(rustc -V))"; cat "$std"
  for id in $ids; do
    echo; echo "$rule"; echo "Rust standard library: $id"; echo; cat "$doc/licenses/$id.txt"
  done
} | tr -d '\r' > "$notices"
rm "$crates" "$std"
echo "license notices written to $notices (Rust in-tree licenses: $(echo $ids))"
