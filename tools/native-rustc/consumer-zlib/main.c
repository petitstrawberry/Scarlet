/* Acceptance test for unmodified zlib 1.3.2 linked against scarlet-libc. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include "zlib.h"

#define DATA_SIZE 131113u
#define PACKED_CAP (DATA_SIZE + 4096u)
#define CHECK(expr) do { \
    if (!(expr)) { \
        printf("SCARLET_ZLIB FAIL line=%d expression=%s errno=%d\n", \
               __LINE__, #expr, errno); \
        return 2; \
    } \
} while (0)

static unsigned char data[DATA_SIZE];
static unsigned char decoded[DATA_SIZE + 1u];
static unsigned char chunked[PACKED_CAP];

static unsigned int smaller(unsigned int a, unsigned int b) {
    return a < b ? a : b;
}

static int core_test(void) {
    unsigned int i;
    unsigned int random = 0x735c91a7u;
    unsigned char *packed;
    uLong bound = compressBound(DATA_SIZE);
    uLongf packed_size = bound;
    uLongf decoded_size = sizeof(decoded);
    unsigned char saved;
    z_stream stream;
    unsigned int supplied;
    unsigned int iterations;
    uLong chunked_size;
    int status;

    for (i = 0; i < DATA_SIZE; ++i) {
        random = random * 1664525u + 1013904223u;
        /* Alternate binary noise and long repeated ranges, including NULs. */
        data[i] = (i / 2048u) % 3u == 0u
            ? (unsigned char)(random >> 24)
            : (unsigned char)((i / 23u) & 15u);
    }
    CHECK(strcmp(zlibVersion(), "1.3.2") == 0);
    packed = malloc((size_t)bound);
    CHECK(packed != NULL);
    CHECK(compress2(packed, &packed_size, data, DATA_SIZE,
                    Z_BEST_COMPRESSION) == Z_OK);
    CHECK(packed_size > 2u && packed_size < bound);
    CHECK(uncompress(decoded, &decoded_size, packed, packed_size) == Z_OK);
    CHECK(decoded_size == DATA_SIZE);
    CHECK(memcmp(decoded, data, DATA_SIZE) == 0);
    saved = packed[0];
    packed[0] = 0; /* Invalid zlib compression-method header. */
    decoded_size = sizeof(decoded);
    CHECK(uncompress(decoded, &decoded_size, packed, packed_size)
          == Z_DATA_ERROR);
    packed[0] = saved;
    free(packed);

    memset(&stream, 0, sizeof(stream));
    CHECK(deflateInit(&stream, Z_DEFAULT_COMPRESSION) == Z_OK);
    supplied = 0;
    iterations = 0;
    do {
        uLong previous_in = stream.total_in;
        uLong previous_out = stream.total_out;
        CHECK(++iterations < 1000000u);
        if (stream.avail_in == 0 && supplied < DATA_SIZE) {
            stream.avail_in = smaller(37u, DATA_SIZE - supplied);
            stream.next_in = data + supplied;
            supplied += stream.avail_in;
        }
        CHECK(stream.total_out < sizeof(chunked));
        stream.next_out = chunked + stream.total_out;
        stream.avail_out = smaller(71u,
            (unsigned int)(sizeof(chunked) - stream.total_out));
        status = deflate(&stream,
            supplied == DATA_SIZE ? Z_FINISH : Z_NO_FLUSH);
        CHECK(status == Z_OK || status == Z_STREAM_END);
        CHECK(status == Z_STREAM_END || stream.total_in != previous_in
              || stream.total_out != previous_out);
    } while (status != Z_STREAM_END);
    CHECK(stream.total_in == DATA_SIZE);
    chunked_size = stream.total_out;
    CHECK(deflateEnd(&stream) == Z_OK);

    memset(decoded, 0xa5, sizeof(decoded));
    memset(&stream, 0, sizeof(stream));
    CHECK(inflateInit(&stream) == Z_OK);
    supplied = 0;
    iterations = 0;
    do {
        uLong previous_in = stream.total_in;
        uLong previous_out = stream.total_out;
        CHECK(++iterations < 1000000u);
        if (stream.avail_in == 0 && supplied < chunked_size) {
            stream.avail_in = smaller(29u,
                (unsigned int)(chunked_size - supplied));
            stream.next_in = chunked + supplied;
            supplied += stream.avail_in;
        }
        CHECK(stream.total_out < sizeof(decoded));
        stream.next_out = decoded + stream.total_out;
        stream.avail_out = smaller(43u,
            (unsigned int)(sizeof(decoded) - stream.total_out));
        status = inflate(&stream, Z_NO_FLUSH);
        CHECK(status == Z_OK || status == Z_STREAM_END);
        CHECK(status == Z_STREAM_END || stream.total_in != previous_in
              || stream.total_out != previous_out);
    } while (status != Z_STREAM_END);
    CHECK(stream.total_in == chunked_size);
    CHECK(stream.total_out == DATA_SIZE);
    CHECK(memcmp(decoded, data, DATA_SIZE) == 0);
    CHECK(decoded[DATA_SIZE] == 0xa5);
    CHECK(inflateEnd(&stream) == Z_OK);
    CHECK(puts("SCARLET_ZLIB CORE PASS") >= 0);
    return 0;
}

static int make_path(char *path, size_t size, const char *directory,
                     const char *name) {
    int length = snprintf(path, size, "%s/%s", directory, name);
    CHECK(length > 0 && (size_t)length < size);
    return 0;
}

static int gzip_test(const char *directory) {
    char path[1024];
    char duplicate_path[1024];
    char missing_path[1024];
    char suffix[64];
    char actual_suffix[64];
    const char *error_message;
    gzFile file;
    int suffix_size;
    int error_code;
    int count;
    int fd;
    int duplicate;
    unsigned int offset;
    unsigned char byte = 0;

    CHECK(make_path(path, sizeof(path), directory, "zlib-roundtrip.gz") == 0);
    CHECK(make_path(duplicate_path, sizeof(duplicate_path), directory,
                    "zlib-duplicate.gz") == 0);
    CHECK(make_path(missing_path, sizeof(missing_path), directory,
                    "zlib-does-not-exist.gz") == 0);
    suffix_size = snprintf(suffix, sizeof(suffix), "%s:%d\n", "scarlet", 132);
    CHECK(suffix_size > 0 && (size_t)suffix_size < sizeof(suffix));

    file = gzopen(path, "wb6");
    CHECK(file != NULL);
    CHECK(gzbuffer(file, 257u) == 0);
    CHECK(gzwrite(file, data, DATA_SIZE) == (int)DATA_SIZE);
    CHECK(gzprintf(file, "%s:%d\n", "scarlet", 132) == suffix_size);
    CHECK(gztell(file) == (z_off_t)(DATA_SIZE + (unsigned int)suffix_size));
    CHECK(gzclose(file) == Z_OK);

    file = gzopen(path, "rb");
    CHECK(file != NULL);
    CHECK(gzbuffer(file, 127u) == 0);
    error_code = -1;
    error_message = gzerror(file, &error_code);
    CHECK(error_message != NULL && error_code == Z_OK);
    CHECK(gzread(file, decoded, 97u) == 97);
    CHECK(memcmp(decoded, data, 97u) == 0);
    CHECK(gztell(file) == 97);
    CHECK(gzseek(file, 65539, SEEK_SET) == 65539);
    CHECK(gztell(file) == 65539);
    CHECK(gzread(file, decoded, 113u) == 113);
    CHECK(memcmp(decoded, data + 65539, 113u) == 0);
    CHECK(gzseek(file, 7, SEEK_SET) == 7);
    CHECK(gztell(file) == 7);
    CHECK(gzread(file, decoded, 211u) == 211);
    CHECK(memcmp(decoded, data + 7, 211u) == 0);
    CHECK(gzseek(file, 31, SEEK_CUR) == 249);
    CHECK(gzread(file, decoded, 53u) == 53);
    CHECK(memcmp(decoded, data + 249, 53u) == 0);
    CHECK(gzseek(file, 0, SEEK_SET) == 0);
    offset = 0;
    while (offset < DATA_SIZE) {
        unsigned int wanted = smaller(503u, DATA_SIZE - offset);
        count = gzread(file, decoded + offset, wanted);
        CHECK(count > 0 && (unsigned int)count <= wanted);
        offset += (unsigned int)count;
    }
    CHECK(memcmp(decoded, data, DATA_SIZE) == 0);
    CHECK(gzread(file, actual_suffix, (unsigned int)suffix_size) == suffix_size);
    CHECK(memcmp(actual_suffix, suffix, (size_t)suffix_size) == 0);
    CHECK(gzread(file, &byte, 1u) == 0);
    CHECK(gzeof(file) != 0);
    error_message = gzerror(file, &error_code);
    CHECK(error_message != NULL && error_code == Z_OK);
    CHECK(gzclose(file) == Z_OK);

    fd = open(duplicate_path, O_WRONLY | O_CREAT | O_TRUNC, 0600);
    CHECK(fd >= 0);
    duplicate = dup(fd);
    CHECK(duplicate >= 0 && duplicate != fd);
    file = gzdopen(duplicate, "wb");
    CHECK(file != NULL);
    CHECK(gzwrite(file, data, 4099u) == 4099);
    CHECK(gzclose(file) == Z_OK);
    errno = 0;
    CHECK(write(duplicate, &byte, 1u) == -1 && errno == EBADF);
    CHECK(write(fd, &byte, 0u) == 0);
    CHECK(close(fd) == 0);
    file = gzopen(duplicate_path, "rb");
    CHECK(file != NULL);
    CHECK(gzread(file, decoded, 4099u) == 4099);
    CHECK(memcmp(decoded, data, 4099u) == 0);
    CHECK(gzread(file, &byte, 1u) == 0);
    CHECK(gzclose(file) == Z_OK);

    errno = 0;
    CHECK(gzopen(missing_path, "rb") == NULL);
    CHECK(errno == ENOENT);

    /* Damage the saved CRC, then require a real gzip error and diagnostic. */
    fd = open(path, O_RDWR);
    CHECK(fd >= 0);
    CHECK(lseek(fd, -8, SEEK_END) >= 0);
    CHECK(read(fd, &byte, 1u) == 1);
    byte ^= 1u;
    CHECK(lseek(fd, -1, SEEK_CUR) >= 0);
    CHECK(write(fd, &byte, 1u) == 1);
    CHECK(close(fd) == 0);
    file = gzopen(path, "rb");
    CHECK(file != NULL);
    offset = 0;
    do {
        count = gzread(file, decoded, sizeof(decoded));
        CHECK(++offset < 16u);
    } while (count > 0);
    CHECK(count == -1);
    error_message = gzerror(file, &error_code);
    CHECK(error_code == Z_DATA_ERROR);
    CHECK(error_message != NULL && error_message[0] != '\0');
    CHECK(gzclose(file) == Z_OK);
    CHECK(puts("SCARLET_ZLIB GZIP PASS") >= 0);
    return 0;
}

int main(int argc, char **argv) {
    CHECK(argc == 2 && argv[1] != NULL && argv[1][0] != '\0');
    CHECK(core_test() == 0);
    CHECK(gzip_test(argv[1]) == 0);
    CHECK(puts("SCARLET_ZLIB 1.3.2 PASS") >= 0);
    CHECK(puts("SCARLET_LIBC_ZLIB_OK") >= 0);
    return 47;
}
