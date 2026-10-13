# The native SDK supports ELF. This bootstrap module only builds archives;
# executable startup and link closure are supplied by the matching SDK.
set(CMAKE_EXECUTABLE_SUFFIX "")
set(CMAKE_FIND_LIBRARY_PREFIXES "lib")
set(CMAKE_FIND_LIBRARY_SUFFIXES ".a" ".so")
