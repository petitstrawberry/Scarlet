#ifndef SCARLET_INTTYPES_H
#define SCARLET_INTTYPES_H

#include <stdint.h>

/* Clang supplies exact target ABI format suffixes for each stdint type. */
#define PRId8 __INT8_FMTd__
#define PRIi8 __INT8_FMTi__
#define PRIo8 __UINT8_FMTo__
#define PRIu8 __UINT8_FMTu__
#define PRIx8 __UINT8_FMTx__
#define PRIX8 __UINT8_FMTX__

#define PRId16 __INT16_FMTd__
#define PRIi16 __INT16_FMTi__
#define PRIo16 __UINT16_FMTo__
#define PRIu16 __UINT16_FMTu__
#define PRIx16 __UINT16_FMTx__
#define PRIX16 __UINT16_FMTX__

#define PRId32 __INT32_FMTd__
#define PRIi32 __INT32_FMTi__
#define PRIo32 __UINT32_FMTo__
#define PRIu32 __UINT32_FMTu__
#define PRIx32 __UINT32_FMTx__
#define PRIX32 __UINT32_FMTX__

#define PRId64 __INT64_FMTd__
#define PRIi64 __INT64_FMTi__
#define PRIo64 __UINT64_FMTo__
#define PRIu64 __UINT64_FMTu__
#define PRIx64 __UINT64_FMTx__
#define PRIX64 __UINT64_FMTX__

#define PRIdLEAST8 __INT_LEAST8_FMTd__
#define PRIiLEAST8 __INT_LEAST8_FMTi__
#define PRIoLEAST8 __UINT_LEAST8_FMTo__
#define PRIuLEAST8 __UINT_LEAST8_FMTu__
#define PRIxLEAST8 __UINT_LEAST8_FMTx__
#define PRIXLEAST8 __UINT_LEAST8_FMTX__

#define PRIdLEAST16 __INT_LEAST16_FMTd__
#define PRIiLEAST16 __INT_LEAST16_FMTi__
#define PRIoLEAST16 __UINT_LEAST16_FMTo__
#define PRIuLEAST16 __UINT_LEAST16_FMTu__
#define PRIxLEAST16 __UINT_LEAST16_FMTx__
#define PRIXLEAST16 __UINT_LEAST16_FMTX__

#define PRIdLEAST32 __INT_LEAST32_FMTd__
#define PRIiLEAST32 __INT_LEAST32_FMTi__
#define PRIoLEAST32 __UINT_LEAST32_FMTo__
#define PRIuLEAST32 __UINT_LEAST32_FMTu__
#define PRIxLEAST32 __UINT_LEAST32_FMTx__
#define PRIXLEAST32 __UINT_LEAST32_FMTX__

#define PRIdLEAST64 __INT_LEAST64_FMTd__
#define PRIiLEAST64 __INT_LEAST64_FMTi__
#define PRIoLEAST64 __UINT_LEAST64_FMTo__
#define PRIuLEAST64 __UINT_LEAST64_FMTu__
#define PRIxLEAST64 __UINT_LEAST64_FMTx__
#define PRIXLEAST64 __UINT_LEAST64_FMTX__

#define PRIdFAST8 __INT_FAST8_FMTd__
#define PRIiFAST8 __INT_FAST8_FMTi__
#define PRIoFAST8 __UINT_FAST8_FMTo__
#define PRIuFAST8 __UINT_FAST8_FMTu__
#define PRIxFAST8 __UINT_FAST8_FMTx__
#define PRIXFAST8 __UINT_FAST8_FMTX__

#define PRIdFAST16 __INT_FAST16_FMTd__
#define PRIiFAST16 __INT_FAST16_FMTi__
#define PRIoFAST16 __UINT_FAST16_FMTo__
#define PRIuFAST16 __UINT_FAST16_FMTu__
#define PRIxFAST16 __UINT_FAST16_FMTx__
#define PRIXFAST16 __UINT_FAST16_FMTX__

#define PRIdFAST32 __INT_FAST32_FMTd__
#define PRIiFAST32 __INT_FAST32_FMTi__
#define PRIoFAST32 __UINT_FAST32_FMTo__
#define PRIuFAST32 __UINT_FAST32_FMTu__
#define PRIxFAST32 __UINT_FAST32_FMTx__
#define PRIXFAST32 __UINT_FAST32_FMTX__

#define PRIdFAST64 __INT_FAST64_FMTd__
#define PRIiFAST64 __INT_FAST64_FMTi__
#define PRIoFAST64 __UINT_FAST64_FMTo__
#define PRIuFAST64 __UINT_FAST64_FMTu__
#define PRIxFAST64 __UINT_FAST64_FMTx__
#define PRIXFAST64 __UINT_FAST64_FMTX__

#define PRIdMAX __INTMAX_FMTd__
#define PRIiMAX __INTMAX_FMTi__
#define PRIoMAX __UINTMAX_FMTo__
#define PRIuMAX __UINTMAX_FMTu__
#define PRIxMAX __UINTMAX_FMTx__
#define PRIXMAX __UINTMAX_FMTX__

#define PRIdPTR __INTPTR_FMTd__
#define PRIiPTR __INTPTR_FMTi__
#define PRIoPTR __UINTPTR_FMTo__
#define PRIuPTR __UINTPTR_FMTu__
#define PRIxPTR __UINTPTR_FMTx__
#define PRIXPTR __UINTPTR_FMTX__

/* scanf uses the same length modifiers and conversions. */
#define SCNd8 PRId8
#define SCNi8 PRIi8
#define SCNo8 PRIo8
#define SCNu8 PRIu8
#define SCNx8 PRIx8

#define SCNd16 PRId16
#define SCNi16 PRIi16
#define SCNo16 PRIo16
#define SCNu16 PRIu16
#define SCNx16 PRIx16

#define SCNd32 PRId32
#define SCNi32 PRIi32
#define SCNo32 PRIo32
#define SCNu32 PRIu32
#define SCNx32 PRIx32

#define SCNd64 PRId64
#define SCNi64 PRIi64
#define SCNo64 PRIo64
#define SCNu64 PRIu64
#define SCNx64 PRIx64

#define SCNdLEAST8 PRIdLEAST8
#define SCNiLEAST8 PRIiLEAST8
#define SCNoLEAST8 PRIoLEAST8
#define SCNuLEAST8 PRIuLEAST8
#define SCNxLEAST8 PRIxLEAST8

#define SCNdLEAST16 PRIdLEAST16
#define SCNiLEAST16 PRIiLEAST16
#define SCNoLEAST16 PRIoLEAST16
#define SCNuLEAST16 PRIuLEAST16
#define SCNxLEAST16 PRIxLEAST16

#define SCNdLEAST32 PRIdLEAST32
#define SCNiLEAST32 PRIiLEAST32
#define SCNoLEAST32 PRIoLEAST32
#define SCNuLEAST32 PRIuLEAST32
#define SCNxLEAST32 PRIxLEAST32

#define SCNdLEAST64 PRIdLEAST64
#define SCNiLEAST64 PRIiLEAST64
#define SCNoLEAST64 PRIoLEAST64
#define SCNuLEAST64 PRIuLEAST64
#define SCNxLEAST64 PRIxLEAST64

#define SCNdFAST8 PRIdFAST8
#define SCNiFAST8 PRIiFAST8
#define SCNoFAST8 PRIoFAST8
#define SCNuFAST8 PRIuFAST8
#define SCNxFAST8 PRIxFAST8

#define SCNdFAST16 PRIdFAST16
#define SCNiFAST16 PRIiFAST16
#define SCNoFAST16 PRIoFAST16
#define SCNuFAST16 PRIuFAST16
#define SCNxFAST16 PRIxFAST16

#define SCNdFAST32 PRIdFAST32
#define SCNiFAST32 PRIiFAST32
#define SCNoFAST32 PRIoFAST32
#define SCNuFAST32 PRIuFAST32
#define SCNxFAST32 PRIxFAST32

#define SCNdFAST64 PRIdFAST64
#define SCNiFAST64 PRIiFAST64
#define SCNoFAST64 PRIoFAST64
#define SCNuFAST64 PRIuFAST64
#define SCNxFAST64 PRIxFAST64

#define SCNdMAX PRIdMAX
#define SCNiMAX PRIiMAX
#define SCNoMAX PRIoMAX
#define SCNuMAX PRIuMAX
#define SCNxMAX PRIxMAX

#define SCNdPTR PRIdPTR
#define SCNiPTR PRIiPTR
#define SCNoPTR PRIoPTR
#define SCNuPTR PRIuPTR
#define SCNxPTR PRIxPTR

typedef struct {
    intmax_t quot;
    intmax_t rem;
} imaxdiv_t;

#ifdef __cplusplus
extern "C" {
#endif
intmax_t imaxabs(intmax_t);
imaxdiv_t imaxdiv(intmax_t, intmax_t);
intmax_t strtoimax(const char *, char **, int);
uintmax_t strtoumax(const char *, char **, int);
#ifdef __cplusplus
}
#endif

#endif
