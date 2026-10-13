# Scarlet exposes the Unix C/POSIX interfaces used by native LLVM runtimes.
# Declaring the target family does not permit host library searches.
set(UNIX TRUE)
# This bootstrap module builds archives. Native executables use Scarlet's CRT
# and the ordinary Scarlet linker.
set(CMAKE_EXECUTABLE_SUFFIX "")
set(CMAKE_FIND_LIBRARY_PREFIXES "lib")
set(CMAKE_FIND_LIBRARY_SUFFIXES ".a" ".so")
