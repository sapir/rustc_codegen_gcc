set -e

export CARGO_INCREMENTAL=0

unamestr=`uname`
if [[ "$unamestr" == 'Linux' ]]; then
   dylib_ext='so'
elif [[ "$unamestr" == 'Darwin' ]]; then
   dylib_ext='dylib'
else
   echo "Unsupported os"
   exit 1
fi

HOST_TRIPLE=$(rustc -vV | grep host | cut -d: -f2 | tr -d " ")
TARGET_TRIPLE=$HOST_TRIPLE
#TARGET_TRIPLE="aarch64-unknown-linux-gnu"

linker=''
RUN_WRAPPER=''
if [[ "$HOST_TRIPLE" != "$TARGET_TRIPLE" ]]; then
   if [[ "$TARGET_TRIPLE" == "aarch64-unknown-linux-gnu" ]]; then
      # We are cross-compiling for aarch64. Use the correct linker and run tests in qemu.
      linker='-Clinker=aarch64-linux-gnu-gcc'
      RUN_WRAPPER='qemu-aarch64 -L /usr/aarch64-linux-gnu'
   else
      echo "Unknown non-native platform"
   fi
fi

export RUSTFLAGS=$linker' -Cpanic=abort -Cdebuginfo=2 -Zpanic-abort-tests -Zcodegen-backend='$(pwd)'/target/'$CHANNEL'/librustc_codegen_gcc.'$dylib_ext' --sysroot '$(pwd)'/build_sysroot/sysroot'
#export RUSTFLAGS=$linker' -Cpanic=abort -Cdebuginfo=2 -Zpanic-abort-tests -Zcodegen-backend='$(pwd)'/target/'$CHANNEL'/librustc_codegen_gcc.'$dylib_ext' --sysroot '$(pwd)'/build_sysroot/sysroot -Clto=fat -Cembed-bitcode=yes'

# FIXME remove once the atomic shim is gone
if [[ `uname` == 'Darwin' ]]; then
   export RUSTFLAGS="$RUSTFLAGS -Clink-arg=-undefined -Clink-arg=dynamic_lookup"
fi

RUSTC="rustc $RUSTFLAGS -L crate=target/out --out-dir target/out"
export RUSTC_LOG=warn # display metadata load errors

export LD_LIBRARY_PATH="$(pwd)/target/out:$(pwd)/build_sysroot/sysroot/lib/rustlib/$TARGET_TRIPLE/lib:$(pwd)/../../../Projets/gcc-build/build/gcc/"
export DYLD_LIBRARY_PATH=$LD_LIBRARY_PATH

export CG_CLIF_DISPLAY_CG_TIME=1
export CG_CLIF_INCR_CACHE_DISABLED=1
