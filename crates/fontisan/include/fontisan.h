/* fontisan C ABI, version 1. Hand-maintained; layout is checked by
 * crates/fontisan/tests/ffi_abi.rs. Any change here bumps FONTISAN_ABI_VERSION. */
#ifndef FONTISAN_H
#define FONTISAN_H
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define FONTISAN_MAGIC 0x4E415346u /* "FSAN" little-endian */
#define FONTISAN_ABI_VERSION 1u

enum {
  FONTISAN_OK = 0,
  FONTISAN_E_NULL = 1,
  FONTISAN_E_BAD_MAGIC = 2,
  FONTISAN_E_ABI_MISMATCH = 3,
  FONTISAN_E_BAD_STRUCT_SIZE = 4,
  FONTISAN_E_UNKNOWN_OP = 5,
  FONTISAN_E_PANIC = 6,
  FONTISAN_E_INVALID_ARG = 7,
  FONTISAN_E_CONVERT_FAILED = 8
};

#define FONTISAN_OP_ECHO 0u
/* input = 16-byte prefix (u32 BE target 0 ttf/1 otf/2 woff/3 woff2, u32 reserved,
 * i64 BE timestamp) + font bytes; payload = converted font. Failure message is
 * "<ruby class>: <message>" with status FONTISAN_E_CONVERT_FAILED. */
#define FONTISAN_OP_CONVERT 1u
/* input = 16-byte prefix (u32 BE member index, 12 reserved) + collection;
 * payload = u32 BE basename length, basename, member bytes. */
#define FONTISAN_OP_EXTRACT 2u

typedef struct {
  uint32_t magic;
  uint32_t abi_version;
  uint32_t struct_size; /* sizeof the enclosing struct */
  uint32_t reserved;
} fontisan_header;

typedef struct {
  fontisan_header header;
  uint32_t op;
  uint32_t flags;
  const uint8_t *input;
  size_t input_len;
} fontisan_request;

typedef struct {
  int32_t code;
  const uint8_t *message; /* UTF-8, not NUL-terminated */
  size_t message_len;
} fontisan_error_row;

typedef struct {
  fontisan_header header;
  int32_t status;
  fontisan_error_row error;
  const uint8_t *payload;
  size_t payload_len;
} fontisan_result;

uint32_t fontisan_abi_version(void);

/* Returns the status. *out always receives a result block (also on error)
 * unless `out` is NULL. Release it with fontisan_result_free; that single
 * call frees every pointer inside it. Never free engine memory otherwise. */
int32_t fontisan_call(const fontisan_request *req, fontisan_result **out);
void fontisan_result_free(fontisan_result *result);

#ifdef __cplusplus
}
#endif
#endif
