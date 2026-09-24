#ifndef SCARLET_ARPA_INET_H
#define SCARLET_ARPA_INET_H

#include <netinet/in.h>

#if defined(__BYTE_ORDER__) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
static inline uint16_t htons(uint16_t x) { return __builtin_bswap16(x); }
static inline uint16_t ntohs(uint16_t x) { return __builtin_bswap16(x); }
static inline uint32_t htonl(uint32_t x) { return __builtin_bswap32(x); }
static inline uint32_t ntohl(uint32_t x) { return __builtin_bswap32(x); }
#else
static inline uint16_t htons(uint16_t x) { return x; }
static inline uint16_t ntohs(uint16_t x) { return x; }
static inline uint32_t htonl(uint32_t x) { return x; }
static inline uint32_t ntohl(uint32_t x) { return x; }
#endif

#ifdef __cplusplus
extern "C" {
#endif
int inet_pton(int, const char *, void *);
const char *inet_ntop(int, const void *, char *, socklen_t);
in_addr_t inet_addr(const char *);
char *inet_ntoa(struct in_addr);
#ifdef __cplusplus
}
#endif

#endif
