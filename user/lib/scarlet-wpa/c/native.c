/* Scarlet OS and driver boundary for the unmodified upstream RSN engine.
 * Engine entry points are serialized by the Rust wrapper. No Linux interfaces.
 */
#include "includes.h"
#include "common.h"
#include "eloop.h"
#include "rsn_supp/wpa.h"
#include "crypto/sha1.h"
#include "native.h"

extern void *swpa_rust_alloc(size_t, size_t);
extern void swpa_rust_free(void *, size_t, size_t);
union allocation_header {
  size_t size;
  max_align_t alignment;
};
void *swpa_alloc(size_t size) {
  if (size > SIZE_MAX - sizeof(union allocation_header))
    return NULL;
  union allocation_header *h =
      swpa_rust_alloc(sizeof(*h) + size, _Alignof(union allocation_header));
  if (!h)
    return NULL;
  h->size = size;
  return h + 1;
}
void swpa_release(void *p) {
  if (!p)
    return;
  union allocation_header *h = (union allocation_header *)p - 1;
  volatile unsigned char *bytes = p;
  for (size_t i = 0; i < h->size; i++)
    bytes[i] = 0;
  size_t total = sizeof(*h) + h->size;
  h->size = 0;
  swpa_rust_free(h, total, _Alignof(union allocation_header));
}
void *swpa_resize(void *p, size_t size) {
  if (!p)
    return swpa_alloc(size);
  void *q = swpa_alloc(size);
  if (!q)
    return NULL;
  union allocation_header *h = (union allocation_header *)p - 1;
  memcpy(q, p, size < h->size ? size : h->size);
  swpa_release(p);
  return q;
}
char *swpa_strdup(const char *s) {
  size_t n = strlen(s) + 1;
  char *p = swpa_alloc(n);
  if (p)
    memcpy(p, s, n);
  return p;
}

#define TIMER_LIMIT 16
struct swpa_timer {
  uint64_t due;
  eloop_timeout_handler handler;
  void *eloop, *user;
};
struct swpa {
  struct wpa_sm *sm;
  struct swpa_callbacks cb;
  enum wpa_states state;
  uint8_t peer[6];
  int failed;
  int authorized;
  unsigned int deauth_reason;
  struct swpa_timer timers[TIMER_LIMIT];
};
static struct swpa *active;
static void failure(struct swpa *s) {
  if (!s->failed) {
    s->failed = 1;
    s->cb.fail(s->cb.ctx);
  }
}
static void set_state(void *ctx, enum wpa_states state) {
  struct swpa *s = ctx;
  s->state = state;
  /* GTK exchange keeps the existing controlled port; failed exchange still
   * invokes failure/deauth and revokes it. Initial authentication stays closed.
   */
  s->authorized =
      state == WPA_COMPLETED || (state == WPA_GROUP_HANDSHAKE && s->authorized);
  if (s->cb.state(s->cb.ctx, s->authorized) < 0)
    failure(s);
}
static enum wpa_states get_state(void *ctx) {
  return ((struct swpa *)ctx)->state;
}
static void deauth(void *ctx, u16 reason) {
  if (((struct swpa *)ctx)->deauth_reason == 0)
    ((struct swpa *)ctx)->deauth_reason = reason;
  failure(ctx);
}
static void reconnect(void *ctx) { failure(ctx); }
static void *network(void *ctx) { return ctx; }
static int bssid(void *ctx, u8 *addr) {
  os_memcpy(addr, ((struct swpa *)ctx)->peer, 6);
  return 0;
}
static int send_frame(void *ctx, const u8 *dest, u16 proto, const u8 *buf,
                      size_t len) {
  struct swpa *s = ctx;
  if (s->failed || proto != 0x888e || os_memcmp(dest, s->peer, 6) != 0 ||
      len > 1024)
    return -1;
  int result = s->cb.send(s->cb.ctx, dest, buf, len);
  if (result < 0)
    failure(s);
  return result;
}
static int key(void *ctx, int link, enum wpa_alg alg, const u8 *addr, int index,
               int tx, const u8 *seq, size_t seq_len, const u8 *value,
               size_t len, enum key_flag flags) {
  struct swpa *s = ctx;
  (void)tx;
  /* Native V1 has no RX-only/next-key activation. Upstream explicitly
   * treats rejection of this optional preinstall as a supported fallback. */
  if (flags & KEY_FLAG_NEXT)
    return -1;
  if (s->failed || link != -1 || (alg != WPA_ALG_CCMP && alg != WPA_ALG_NONE) ||
      seq_len > 6)
    return -1;
  int pairwise = (flags & KEY_FLAG_PAIRWISE) != 0;
  int result =
      s->cb.key(s->cb.ctx, alg == WPA_ALG_NONE ? 0 : (pairwise ? 1 : 2), addr,
                index, seq, seq_len, value, len);
  if (result < 0)
    failure(s);
  return result;
}
static int beacon(void *ctx) {
  (void)ctx;
  return 0;
}
static void cancel_auth(void *ctx) { (void)ctx; }
static int protect(void *ctx, const u8 *addr, int type, int keytype) {
  (void)ctx;
  (void)addr;
  (void)type;
  (void)keytype;
  return 0;
}
static u8 *alloc_eapol(void *ctx, u8 type, const void *data, u16 length,
                       size_t *total, void **pos) {
  (void)ctx;
  struct ieee802_1x_hdr *hdr = os_zalloc(sizeof(*hdr) + length);
  if (!hdr)
    return NULL;
  hdr->version = 2;
  hdr->type = type;
  hdr->length = host_to_be16(length);
  *total = sizeof(*hdr) + length;
  *pos = hdr + 1;
  if (data)
    os_memcpy(hdr + 1, data, length);
  return (u8 *)hdr;
}
static int add_pmkid(void *ctx, void *net, const u8 *bssid, const u8 *pmkid,
                     const u8 *fils, const u8 *pmk, size_t len, u32 life,
                     u8 threshold, int akm) {
  (void)ctx;
  (void)net;
  (void)bssid;
  (void)pmkid;
  (void)fils;
  (void)pmk;
  (void)len;
  (void)life;
  (void)threshold;
  (void)akm;
  return -1;
}
static int remove_pmkid(void *ctx, void *net, const u8 *bssid, const u8 *pmkid,
                        const u8 *fils) {
  (void)ctx;
  (void)net;
  (void)bssid;
  (void)pmkid;
  (void)fils;
  return 0;
}
int os_memcmp_const(const void *a, const void *b, size_t len) {
  const unsigned char *x = a, *y = b;
  volatile unsigned char result = 0;
  for (size_t i = 0; i < len; i++)
    result |= x[i] ^ y[i];
  return result;
}
int os_get_random(unsigned char *buf, size_t len) {
  return active ? active->cb.random(active->cb.ctx, buf, len) : -1;
}
int os_get_reltime(struct os_reltime *t) {
  if (!active)
    return -1;
  uint64_t n = active->cb.now(active->cb.ctx);
  t->sec = n / 1000000000;
  t->usec = (n % 1000000000) / 1000;
  return 0;
}
int os_get_time(struct os_time *t) {
  struct os_reltime r;
  if (os_get_reltime(&r))
    return -1;
  t->sec = r.sec;
  t->usec = r.usec;
  return 0;
}
void *os_memdup(const void *source, size_t len) {
  void *p = swpa_alloc(len);
  if (p)
    memcpy(p, source, len);
  return p;
}
void *os_zalloc(size_t len) {
  void *p = swpa_alloc(len);
  if (p)
    memset(p, 0, len);
  return p;
}
int eloop_register_timeout(unsigned int sec, unsigned int usec,
                           eloop_timeout_handler handler, void *eloop,
                           void *user) {
  if (!active)
    return -1;
  for (int i = 0; i < TIMER_LIMIT; i++)
    if (!active->timers[i].handler) {
      active->timers[i] = (struct swpa_timer){active->cb.now(active->cb.ctx) +
                                                  (uint64_t)sec * 1000000000 +
                                                  (uint64_t)usec * 1000,
                                              handler, eloop, user};
      return 0;
    }
  failure(active);
  return -1;
}
int eloop_cancel_timeout(eloop_timeout_handler handler, void *eloop,
                         void *user) {
  int count = 0;
  if (!active)
    return 0;
  for (int i = 0; i < TIMER_LIMIT; i++) {
    struct swpa_timer *t = &active->timers[i];
    if (t->handler == handler &&
        (eloop == ELOOP_ALL_CTX || t->eloop == eloop) &&
        (user == ELOOP_ALL_CTX || t->user == user)) {
      t->handler = NULL;
      count++;
    }
  }
  return count;
}
int eloop_is_timeout_registered(eloop_timeout_handler h, void *e, void *u) {
  if (!active)
    return 0;
  for (int i = 0; i < TIMER_LIMIT; i++)
    if (active->timers[i].handler == h && active->timers[i].eloop == e &&
        active->timers[i].user == u)
      return 1;
  return 0;
}
static int parse_ap_security_ies(const u8 *ies, size_t len,
                                 const u8 **rsne, size_t *rsne_len,
                                 const u8 **rsnxe, size_t *rsnxe_len) {
  size_t pos = 0;
  int seen_rsne = 0;
  int seen_rsnxe = 0;
  *rsne = NULL; *rsne_len = 0;
  *rsnxe = NULL; *rsnxe_len = 0;
  if (!ies || len == 0 || len > 64) return -1;
  while (pos < len) {
    size_t total;
    if (len - pos < 2) return -1;
    total = 2 + ies[pos + 1];
    if (total > len - pos) return -1;
    if (ies[pos] == WLAN_EID_RSN) {
      if (seen_rsne) return -1;
      seen_rsne = 1; *rsne = ies + pos; *rsne_len = total;
    } else if (ies[pos] == WLAN_EID_RSNX) {
      if (seen_rsnxe) return -1;
      seen_rsnxe = 1; *rsnxe = ies + pos; *rsnxe_len = total;
    } else {
      return -1;
    }
    pos += total;
  }
  return seen_rsne ? 0 : -1;
}

struct swpa *swpa_new(const struct swpa_callbacks *cb, const u8 *own,
                      const u8 *peer, const u8 *ssid, size_t ssid_len,
                      const u8 *pmk, const u8 *own_ie, size_t own_len,
                      const u8 *ap_ie, size_t ap_len) {
  struct wpa_ie_data own_rsn, ap_rsn;
  const u8 *ap_rsne, *ap_rsnxe;
  size_t ap_rsne_len, ap_rsnxe_len;
  if (ssid_len == 0 || ssid_len > 32 || own_len > 64 || ap_len > 64 ||
      wpa_parse_wpa_ie_rsn(own_ie, own_len, &own_rsn) < 0 ||
      parse_ap_security_ies(ap_ie, ap_len, &ap_rsne, &ap_rsne_len,
                            &ap_rsnxe, &ap_rsnxe_len) < 0 ||
      wpa_parse_wpa_ie_rsn(ap_rsne, ap_rsne_len, &ap_rsn) < 0 ||
      own_rsn.group_cipher != WPA_CIPHER_CCMP ||
      own_rsn.pairwise_cipher != WPA_CIPHER_CCMP ||
      own_rsn.key_mgmt != WPA_KEY_MGMT_PSK ||
      ap_rsn.group_cipher != WPA_CIPHER_CCMP ||
      !(ap_rsn.pairwise_cipher & WPA_CIPHER_CCMP) ||
      !(ap_rsn.key_mgmt & WPA_KEY_MGMT_PSK) ||
      (ap_rsn.capabilities & WPA_CAPABILITY_MFPR))
    return NULL;
  struct swpa *s = os_zalloc(sizeof(*s));
  if (!s)
    return NULL;
  s->cb = *cb;
  s->state = WPA_ASSOCIATED;
  os_memcpy(s->peer, peer, 6);
  active = s;
  struct wpa_sm_ctx *ctx = os_zalloc(sizeof(*ctx));
  if (!ctx) {
    os_free(s);
    active = NULL;
    return NULL;
  }
  ctx->ctx = s;
  ctx->msg_ctx = s;
  ctx->set_state = set_state;
  ctx->get_state = get_state;
  ctx->deauthenticate = deauth;
  ctx->reconnect = reconnect;
  ctx->set_key = key;
  ctx->get_network_ctx = network;
  ctx->get_bssid = bssid;
  ctx->ether_send = send_frame;
  ctx->get_beacon_ie = beacon;
  ctx->cancel_auth_timeout = cancel_auth;
  ctx->alloc_eapol = alloc_eapol;
  ctx->add_pmkid = add_pmkid;
  ctx->remove_pmkid = remove_pmkid;
  ctx->mlme_setprotection = protect;
  s->sm = wpa_sm_init(ctx);
  if (!s->sm) {
    os_free(ctx);
    os_free(s);
    active = NULL;
    return NULL;
  }
  struct rsn_supp_config config = {0};
  config.network_ctx = s;
  config.allowed_pairwise_cipher = WPA_CIPHER_CCMP;
  config.ssid = ssid;
  config.ssid_len = ssid_len;
  config.wpa_deny_ptk0_rekey = 1;
  wpa_sm_set_config(s->sm, &config);
  wpa_sm_set_own_addr(s->sm, own);
  wpa_sm_set_fast_reauth(s->sm, 0);
  wpa_sm_set_param(s->sm, WPA_PARAM_PROTO, WPA_PROTO_RSN);
  wpa_sm_set_param(s->sm, WPA_PARAM_PAIRWISE, WPA_CIPHER_CCMP);
  wpa_sm_set_param(s->sm, WPA_PARAM_GROUP, WPA_CIPHER_CCMP);
  wpa_sm_set_param(s->sm, WPA_PARAM_KEY_MGMT, WPA_KEY_MGMT_PSK);
  wpa_sm_set_param(s->sm, WPA_PARAM_RSN_ENABLED, 1);
  wpa_sm_set_param(s->sm, WPA_PARAM_DENY_PTK0_REKEY, 1);
  if (wpa_sm_set_assoc_wpa_ie(s->sm, own_ie, own_len) < 0 ||
      wpa_sm_set_ap_rsn_ie(s->sm, ap_rsne, ap_rsne_len) < 0 ||
      (ap_rsnxe && wpa_sm_set_ap_rsnxe(s->sm, ap_rsnxe, ap_rsnxe_len) < 0)) {
    swpa_free(s);
    return NULL;
  }
  wpa_sm_set_pmk(s->sm, pmk, 32, NULL, peer);
  wpa_sm_notify_assoc(s->sm, peer);
  active = NULL;
  return s;
}
int swpa_rx(struct swpa *s, const u8 *peer, const u8 *buf, size_t len,
            int protected_frame) {
  active = s;
  if (s->failed || os_memcmp(peer, s->peer, 6) != 0) {
    active = NULL;
    return -1;
  }
  int result =
      wpa_sm_rx_eapol(s->sm, peer, buf, len,
                      protected_frame ? FRAME_ENCRYPTED : FRAME_NOT_ENCRYPTED);
  if (result == -1)
    failure(s);
  active = NULL;
  return s->failed ? -1 : result;
}
uint64_t swpa_next_deadline(struct swpa *s) {
  uint64_t next = UINT64_MAX;
  for (int i = 0; i < TIMER_LIMIT; i++)
    if (s->timers[i].handler && s->timers[i].due < next)
      next = s->timers[i].due;
  return next;
}
int swpa_tick(struct swpa *s) {
  active = s;
  uint64_t now = s->cb.now(s->cb.ctx);
  for (int i = 0; i < TIMER_LIMIT; i++)
    if (s->timers[i].handler && s->timers[i].due <= now) {
      struct swpa_timer t = s->timers[i];
      s->timers[i].handler = NULL;
      t.handler(t.eloop, t.user);
    }
  active = NULL;
  return s->failed ? -1 : 0;
}
unsigned int swpa_deauth_reason(struct swpa *s) { return s->deauth_reason; }
void swpa_free(struct swpa *s) {
  if (!s)
    return;
  active = s;
  if (s->sm)
    wpa_sm_deinit(s->sm);
  forced_memzero(s, sizeof(*s));
  os_free(s);
  active = NULL;
}
int swpa_pmk(const char *pass, const u8 *ssid, size_t len, u8 *pmk) {
  return pbkdf2_sha1(pass, (const char *)ssid, len, 4096, pmk, 32);
}
