#!/bin/bash
# usage: dumpbin.sh <args...>   (wraps MSVC dumpbin, disabling MSYS path mangling of /flags)
export MSYS2_ARG_CONV_EXCL='*'
exec "/c/Program Files/Microsoft Visual Studio/2022/Community/VC/Tools/MSVC/14.44.35207/bin/Hostx64/x64/dumpbin.exe" "$@"
