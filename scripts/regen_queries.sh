#!/bin/sh
set -e
cd "$(dirname "$0")/.."
NV=${1:?nvim-treesitter queries dir with c.scm cpp.scm ecma.scm jsx.scm javascript.scm typescript.scm go.scm}
CR=$(ls -d "$HOME"/.cargo/registry/src/index.crates.io-*)
Q=assets/queries
A="python3 -I scripts/adapt_queries.py"
$A $Q/swift-highlights.scm "$CR"/tree-sitter-swift-0.7.4/queries/highlights.scm
$A $Q/java-highlights.scm "$CR"/tree-sitter-java-0.23.5/queries/highlights.scm
$A $Q/c-highlights.scm "$NV"/c.scm
$A $Q/cpp-highlights.scm "$NV"/c.scm "$NV"/cpp.scm
$A $Q/go-highlights.scm "$NV"/go.scm
$A $Q/javascript-highlights.scm "$NV"/ecma.scm "$NV"/jsx.scm "$NV"/javascript.scm
$A $Q/typescript-highlights.scm "$NV"/ecma.scm "$NV"/typescript.scm
$A $Q/tsx-highlights.scm "$NV"/ecma.scm "$NV"/jsx.scm "$NV"/typescript.scm
python3 -I scripts/adapt_queries.py $Q/objc-highlights.scm "$NV"/c.scm "$NV"/objc.scm
