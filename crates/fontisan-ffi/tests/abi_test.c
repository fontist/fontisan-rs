/* C-side ABI test (test-only). Opens the ABI, checks a stale header fails loudly. */
#include <stdio.h>
#include <string.h>
#include "fontisan.h"

#define CHECK(c) do { if (!(c)) { fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #c); return 1; } } while (0)

static fontisan_request make(uint32_t op, const uint8_t *in, size_t n) {
  fontisan_request r;
  memset(&r, 0, sizeof r);
  r.header.magic = FONTISAN_MAGIC;
  r.header.abi_version = FONTISAN_ABI_VERSION;
  r.header.struct_size = (uint32_t)sizeof r;
  r.op = op; r.input = in; r.input_len = n;
  return r;
}

int main(void) {
  CHECK(fontisan_abi_version() == FONTISAN_ABI_VERSION);

  const uint8_t data[] = "hello";
  fontisan_request req = make(FONTISAN_OP_ECHO, data, 5);
  fontisan_result *res = NULL;
  CHECK(fontisan_call(&req, &res) == FONTISAN_OK);
  CHECK(res && res->payload_len == 5 && memcmp(res->payload, "hello", 5) == 0);
  CHECK(res->header.magic == FONTISAN_MAGIC && res->header.struct_size == sizeof *res);
  fontisan_result_free(res);

  /* Stale header: older ABI version must fail loudly with a message. */
  req = make(FONTISAN_OP_ECHO, data, 5);
  req.header.abi_version = FONTISAN_ABI_VERSION - 1;
  res = NULL;
  CHECK(fontisan_call(&req, &res) == FONTISAN_E_ABI_MISMATCH);
  CHECK(res && res->status == FONTISAN_E_ABI_MISMATCH && res->error.message_len > 0);
  printf("stale header rejected: %.*s\n", (int)res->error.message_len, (const char *)res->error.message);
  fontisan_result_free(res);

  /* A caller compiled against a smaller struct is rejected, not read past. */
  req = make(FONTISAN_OP_ECHO, data, 5);
  req.header.struct_size -= 8;
  CHECK(fontisan_call(&req, &res) == FONTISAN_E_BAD_STRUCT_SIZE);
  fontisan_result_free(res);

  fontisan_result_free(NULL);
  puts("c abi test ok");
  return 0;
}
