#ifndef SCARLET_UCHAR_H
#define SCARLET_UCHAR_H
#include <stddef.h>
#include <stdint.h>
#include <bits/types/mbstate_t.h>

#ifndef __cplusplus
typedef uint_least16_t char16_t;
typedef uint_least32_t char32_t;
#endif

/* Native type declarations only. mbstate_t retains its existing reserved
 * layout; mbrtoc16/mbrtoc32/c16rtomb/c32rtomb are not implemented. */
#endif
