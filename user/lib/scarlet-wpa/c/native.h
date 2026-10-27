#ifndef SCARLET_WPA_NATIVE_H
#define SCARLET_WPA_NATIVE_H
#include <stddef.h>
#include <stdint.h>
struct swpa_callbacks {
  void *ctx;
  int (*random)(void *, uint8_t *, size_t);
  uint64_t (*now)(void *);
  int (*send)(void *, const uint8_t *, const uint8_t *, size_t);
  int (*key)(void *, int, const uint8_t *, int, const uint8_t *, size_t,
             const uint8_t *, size_t);
  int (*state)(void *, int);
  void (*fail)(void *);
};
struct swpa;
struct swpa *swpa_new(const struct swpa_callbacks *, const uint8_t *,
                      const uint8_t *, const uint8_t *, size_t, const uint8_t *,
                      const uint8_t *, size_t, const uint8_t *, size_t);
int swpa_rx(struct swpa *, const uint8_t *, const uint8_t *, size_t, int);
int swpa_tick(struct swpa *);
uint64_t swpa_next_deadline(struct swpa *);
void swpa_free(struct swpa *);
int swpa_pmk(const char *, const uint8_t *, size_t, uint8_t *);
#endif
