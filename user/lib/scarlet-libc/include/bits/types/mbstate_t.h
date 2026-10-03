#ifndef SCARLET_MBSTATE_T_H
#define SCARLET_MBSTATE_T_H
#include <stdint.h>
/* Reserved native conversion state. This defines stream-position storage only;
 * wide-character/locale conversion entry points are not provided yet. */
typedef struct { uint32_t value, remaining, minimum; } mbstate_t;
#endif
