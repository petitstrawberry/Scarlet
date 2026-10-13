#ifndef SCARLET_ALLOCA_H
#define SCARLET_ALLOCA_H

/* Stack storage remains valid until the calling function returns. */
#define alloca(size) __builtin_alloca((size))

#endif
