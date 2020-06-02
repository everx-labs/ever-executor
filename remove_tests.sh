#!/bin/bash
rm -f version.json
rm -f tonVersion.js
rm -f Dockerfile
rm -f Jenkinsfile
rm -f Makefile
rm -rf src/tests
rm -rf real_ton_boc
rm -rf target
sed -i '/#\[cfg(test)\]/,/extern crate pretty_assertions;/d' src/lib.rs
find . -name tests -type d -print0 | xargs -0 rm -fr
find . -name '*.rs' -type f -print0 | xargs -0 sed -i '/#\[cfg(test)\]/,/mod tests/d'
find . -name '*.rs' -type f -print0 | xargs -0 sed -i -e '/logger::init/d' -e '/use tvm::logger/d'
find . -name '*.rs' -type f -print0 | xargs -0 sed -i -e '/#\[cfg(test)\]/,/mod tests/d' -e '/#\[cfg(feature = "full")\]/d' -e '/logger::init/d' -e '/use tvm::logger/d'
sed -i '/log4rs =/d' Cargo.toml
sed -i '/pretty_assertions =/d' Cargo.toml
sed -i '/libloading =/d' Cargo.toml
sed -i '/parking_lot =/d' Cargo.toml
sed -i '/\[features\]/,$d' Cargo.toml
sed -i 's/build =.*$/build = \"build.rs\"/' Cargo.toml
sed -i 's/clap =.*$/clap = \"2.33.0\"/' Cargo.toml
sed -i 's/zip =.*$/zip = \"0.5.3\"/' Cargo.toml
sed -i 's/full = .*$/full = []/' Cargo.toml
sed -i 's@tonlabs/ton-types@tonlabs/ton-labs-types@g' Cargo.toml
sed -i 's@tonlabs/ton-block@tonlabs/ton-labs-block@g' Cargo.toml
sed -i 's@tonlabs/ton-vm@tonlabs/ton-labs-vm@g' Cargo.toml
sed -i 's/\[dev-dependencies\]//' Cargo.toml

sed -i '/extern crate log4rs/d' src/lib.rs
sed -i '/extern crate parking_lot/d' src/lib.rs
sed -i '/pub mod logger/d' src/lib.rs
sed -i '/#\[cfg(feature = \"use_test_framework\")\]/,/pub mod test_framework;/d' src/lib.rs
sed -i '/# To Test:/,/3. Old way to do nothing new but no control/d' README.md
rm -f $0