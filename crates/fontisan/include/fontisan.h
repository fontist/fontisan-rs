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
  FONTISAN_E_CONVERT_FAILED = 8,
  /* read / audit failed: the error message is JSON {"class","message","causes":[{"class","message"}]},
   * the Ruby error surface (audit load failures are wrapped as BaseCommand does). */
  FONTISAN_E_READ_FAILED = 9,
  /* dfont / Type 1: not read natively; serve the file with the Ruby engine. */
  FONTISAN_E_UNSUPPORTED = 10
};

/* Error kinds: fontisan_result.error_kind, 0 on success. Switch on this, not on the message.
 * Append only; values never change. Added after ABI 1 shipped without a bump: the result block
 * is engine-allocated and read-only, so callers built against the 64-byte result still work;
 * header.struct_size >= 72 (64-bit) says the field is present. */
enum {
  FONTISAN_KIND_NONE = 0,
  FONTISAN_KIND_INVALID_FONT = 1,        /* not a readable font / malformed structure */
  FONTISAN_KIND_INVALID_AXIS = 2,        /* unknown variation axis tag */
  FONTISAN_KIND_INVALID_COORDINATES = 3, /* axis value out of range or not finite */
  FONTISAN_KIND_IO = 4,                  /* file-system / I/O failure */
  FONTISAN_KIND_UNSUPPORTED = 5,         /* not served natively; caller serves it */
  FONTISAN_KIND_SUBSETTING = 6,
  FONTISAN_KIND_CORRUPTED_TABLE = 7,
  FONTISAN_KIND_MISSING_TABLE = 8,
  FONTISAN_KIND_INVALID_ARGUMENT = 9,
  FONTISAN_KIND_TRUNCATED = 10,          /* data ended early (EOFError / IOError) */
  FONTISAN_KIND_OTHER = 11               /* Fontisan::Error, panic, ...: use the message */
};

#define FONTISAN_OP_ECHO 0u
/* input = 16-byte prefix (u32 BE target 0 ttf/1 otf/2 woff/3 woff2, u32 reserved,
 * i64 BE timestamp) + font bytes; payload = converted font. Failure message is
 * "<ruby class>: <message>" with status FONTISAN_E_CONVERT_FAILED. */
#define FONTISAN_OP_CONVERT 1u
/* input = 16-byte prefix (u32 BE member index, 12 reserved) + collection;
 * payload = u32 BE basename length, basename, member bytes. */
#define FONTISAN_OP_EXTRACT 2u
/* Encode a single-font sfnt as WOFF2. input = 8-byte little-endian i64 Unix timestamp, then the
 * sfnt bytes. flags bit 0: use the timestamp for head.modified (else the clock); flags bits
 * 8..15: Brotli quality + 1 (0 = default 11). The result payload is the WOFF2 file. */
#define FONTISAN_OP_WOFF2_ENCODE 3u
#define FONTISAN_WOFF2_FLAG_TIMESTAMP 1u
/* input = u32 BE axis count N, N x (4-byte axis tag, i32 BE 16.16 user value),
 * then the font bytes (ttf/woff/woff2); axes left out take their fvar default.
 * payload = the static instance (gvar fonts only), as Ruby Variable::Instancer#instance.
 * Failure message is "<ruby class>: <message>" with FONTISAN_E_CONVERT_FAILED. */
#define FONTISAN_OP_INSTANCE 7u
/* input = 16-byte prefix (u32 BE profile 0 pdf/1 web/2 minimal, u32 BE glyph
 * count N, 8 reserved) + N u32 BE glyph ids + font bytes; payload = the subset
 * font. Failure message is "<ruby class>: <message>" with status
 * FONTISAN_E_CONVERT_FAILED. */
#define FONTISAN_OP_SUBSET 6u

/* Read-side ops. Every success payload is "packed rows": u32 BE row count, then per row a u32 BE
 * byte length and that many bytes of UTF-8 JSON (the facade's canonical documents).
 *
 * READ   input = font bytes; flags: FONTISAN_READ_METADATA (else full), FONTISAN_READ_LAZY.
 *        One row per face (Reference.read). Fails whole on the first error.
 * AUDIT  input = u32 BE name length, source file name, font bytes; flags: FONTISAN_AUDIT_VALIDATE.
 *        One row per face (audit report; nil and empty lists omitted).
 * SCAN   input = UTF-8 file or directory path (directory: recursive, sorted by path, extensions
 *        ttf otf ttc otc woff woff2 dfont pfb pfa). One row per scan row; status "unsupported"
 *        rows (dfont, Type 1) are for the caller to serve.
 * AUDIT_DIR  same input; flags: FONTISAN_AUDIT_VALIDATE. One row per file:
 *        {path, status ok|unsupported|error, reports | error:{class,message,causes}}. */
#define FONTISAN_OP_READ 8u
#define FONTISAN_OP_AUDIT 9u
#define FONTISAN_OP_SCAN 10u
#define FONTISAN_OP_AUDIT_DIR 11u
#define FONTISAN_READ_METADATA 1u
#define FONTISAN_READ_LAZY 2u
#define FONTISAN_AUDIT_VALIDATE 1u

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
  int32_t error_kind; /* FONTISAN_KIND_*; present when header.struct_size >= 72 */
  int32_t reserved;
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
