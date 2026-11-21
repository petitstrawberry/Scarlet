// Freestanding Scarlet AArch64 PIE. Run through the installed /bin/scarlet-ld.
typedef unsigned long word;
static word call(word n, word a, word b, word c) {
  register word x0 __asm__("x0") = a;
  register word x1 __asm__("x1") = b;
  register word x2 __asm__("x2") = c;
  register word x3 __asm__("x3") = 0;
  register word x4 __asm__("x4") = 0;
  register word x8 __asm__("x8") = n;
  __asm__ volatile("svc #0"
                   : "+r"(x0)
                   : "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x8)
                   : "memory", "cc");
  return x0;
}
#define PRINT(s) call(201, 1, (word)(s), sizeof(s) - 1)
__attribute__((noreturn)) static void finish(word status) {
  call(23, status, 0, 0);
  __builtin_trap();
}
static void require(int ok, word line) {
  if (!ok) {
    char msg[] = "QUARTER FAIL line 000\n";
    msg[18] += line / 100;
    msg[19] += line / 10 % 10;
    msg[20] += line % 10;
    call(201, 1, (word)msg, sizeof(msg) - 1);
    finish(1);
  }
}
#define CHECK(expr) require((expr), __LINE__)
static volatile unsigned char tile[16384] __attribute__((aligned(16384))) = {
    11, [4096] = 22, [8192] = 33, [12288] = 44};
#ifdef TEST_RELRO
// The harness places PT_GNU_RELRO over only the first 4 KiB of this pair.
static volatile unsigned char relro_pair[8192]
    __attribute__((aligned(16384))) = {3, [4096] = 4};
#endif

enum {
  Clone = 2,
  Waitpid = 5,
  MemoryUnmap = 701,
  MemoryProtect = 702,
  MemoryQuery = 703
};
static void expect_denied(volatile unsigned char *address, int store) {
  word child = call(Clone, 0, 0, 0);
  CHECK(child != (word)-1);
  if (child == 0) {
    if (store)
      *address = 0xaa;
    else
      (void)*address;
    finish(71);
  }
  int status = -1;
  CHECK(call(Waitpid, child, (word)&status, 0) == child);
  CHECK(status == 139);
}
__attribute__((noreturn)) void _start(word argc, word *argv) {
  word *env = argv + argc + 1;
  while (*env++) {
  }
  word pagesz = 0;
  for (; env[0]; env += 2)
    if (env[0] == 6)
      pagesz = env[1];
  if (pagesz == 16384)
    PRINT("AT_PAGESZ=16384\n");
  if (pagesz == 4096)
    PRINT("AT_PAGESZ=4096\n");
  CHECK(pagesz == EXPECT_PAGESZ);
  CHECK(((word)tile & 16383) == 0);
  CHECK(tile[0] == 11 && tile[4096] == 22 && tile[8192] == 33 &&
        tile[12288] == 44);
  tile[4096] = 55;
  CHECK(call(MemoryProtect, (word)tile + 4096, 4096, 1) == 0);
  tile[0] = 65;
  tile[8192] = 66;
  tile[12288] = 67;
  CHECK(tile[0] == 65 && tile[4096] == 55 && tile[8192] == 66 &&
        tile[12288] == 67);
  CHECK(call(MemoryUnmap, (word)tile + 8192, 4096, 0) == 0);
  tile[0]++;
  tile[12288]++;
  CHECK(tile[0] == 66 && tile[4096] == 55 && tile[12288] == 68);
  word bounds[2];
  CHECK(call(MemoryQuery, (word)tile + 8192, (word)bounds, 0) == (word)-1);
  expect_denied(tile + 4096, 1);
  expect_denied(tile + 8192, 0);
#ifdef TEST_RELRO
  CHECK(relro_pair[0] == 3 && relro_pair[4096] == 4);
  relro_pair[4096] = 77;
  expect_denied(relro_pair, 1);
  CHECK(relro_pair[0] == 3 && relro_pair[4096] == 77);
#endif
  CHECK(tile[0] == 66 && tile[4096] == 55 && tile[12288] == 68);
  PRINT("QUARTER PASS\n");
  finish(0);
}
