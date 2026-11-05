/* C-side ABI test (test-only). Usage: abi_test <corpus fonts dir> <mixed dir>.
 * <mixed dir> holds TestTTF.ttf, TestTTC.ttc (2 faces), TestDFONT.dfont, TestT1-Regular.pfb and a
 * README.txt. Exact golden parity (every read/audit/scan/malformed golden) is
 * crates/fontisan/tests/ffi_read_ops.rs, through this same ABI; this file proves a real C caller. */
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "fontisan.h"

#define CHECK(c) do { if (!(c)) { fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #c); exit(1); } } while (0)

static fontisan_request make(uint32_t op, const uint8_t *in, size_t n) {
  fontisan_request r;
  memset(&r, 0, sizeof r);
  r.header.magic = FONTISAN_MAGIC;
  r.header.abi_version = FONTISAN_ABI_VERSION;
  r.header.struct_size = (uint32_t)sizeof r;
  r.op = op; r.input = in; r.input_len = n;
  return r;
}

static int contains(const uint8_t *hay, size_t n, const char *needle) {
  size_t m = strlen(needle);
  for (size_t i = 0; i + m <= n; i++) if (memcmp(hay + i, needle, m) == 0) return 1;
  return 0;
}

static uint32_t be32(const uint8_t *p) {
  return ((uint32_t)p[0] << 24) | ((uint32_t)p[1] << 16) | ((uint32_t)p[2] << 8) | p[3];
}

/* Walks a packed-rows payload; returns the row count, or -1 when it is malformed. */
static long rows_in(const fontisan_result *res) {
  if (res->payload_len < 4) return -1;
  uint32_t n = be32(res->payload);
  size_t at = 4;
  for (uint32_t i = 0; i < n; i++) {
    if (at + 4 > res->payload_len) return -1;
    at += 4 + be32(res->payload + at);
  }
  return at == res->payload_len ? (long)n : -1;
}

static uint8_t *slurp(const char *dir, const char *name, size_t *len) {
  char path[1024];
  snprintf(path, sizeof path, "%s/%s", dir, name);
  FILE *f = fopen(path, "rb");
  if (!f) { fprintf(stderr, "cannot open %s\n", path); exit(1); }
  fseek(f, 0, SEEK_END);
  long n = ftell(f);
  fseek(f, 0, SEEK_SET);
  uint8_t *b = malloc((size_t)n + 1);
  CHECK(fread(b, 1, (size_t)n, f) == (size_t)n);
  fclose(f);
  *len = (size_t)n;
  return b;
}

static fontisan_result *run(uint32_t op, uint32_t flags, const uint8_t *in, size_t n, int32_t *status) {
  fontisan_request req = make(op, in, n);
  req.flags = flags;
  fontisan_result *res = NULL;
  *status = fontisan_call(&req, &res);
  CHECK(res != NULL);
  return res;
}

static uint8_t *audit_input(const char *name, const uint8_t *font, size_t n, size_t *out_len) {
  size_t l = strlen(name);
  uint8_t *b = malloc(4 + l + n);
  b[0] = (uint8_t)(l >> 24); b[1] = (uint8_t)(l >> 16); b[2] = (uint8_t)(l >> 8); b[3] = (uint8_t)l;
  memcpy(b + 4, name, l);
  memcpy(b + 4 + l, font, n);
  *out_len = 4 + l + n;
  return b;
}

static const char *g_fonts, *g_mixed;

/* Every op on the same inputs; the payloads are copied to `out` so threads can compare them. */
#define SNAP_MAX 8
typedef struct { uint8_t *data[SNAP_MAX]; size_t len[SNAP_MAX]; } snapshot;

static void snap_one(snapshot *s, int i, fontisan_result *res, int32_t status) {
  CHECK(status == FONTISAN_OK);
  s->len[i] = res->payload_len;
  s->data[i] = malloc(res->payload_len);
  memcpy(s->data[i], res->payload, res->payload_len);
  fontisan_result_free(res);
}

static void take_snapshot(snapshot *s) {
  size_t n, an;
  int32_t st;
  uint8_t *ttf = slurp(g_fonts, "TestTTF.ttf", &n);
  uint8_t *ain = audit_input("TestTTF.ttf", ttf, n, &an);
  fontisan_result *r;
  r = run(FONTISAN_OP_READ, 0, ttf, n, &st); snap_one(s, 0, r, st);
  r = run(FONTISAN_OP_READ, FONTISAN_READ_METADATA | FONTISAN_READ_LAZY, ttf, n, &st); snap_one(s, 1, r, st);
  r = run(FONTISAN_OP_AUDIT, FONTISAN_AUDIT_VALIDATE, ain, an, &st); snap_one(s, 2, r, st);
  r = run(FONTISAN_OP_SCAN, 0, (const uint8_t *)g_mixed, strlen(g_mixed), &st); snap_one(s, 3, r, st);
  r = run(FONTISAN_OP_AUDIT_DIR, 0, (const uint8_t *)g_mixed, strlen(g_mixed), &st); snap_one(s, 4, r, st);
  free(ttf); free(ain);
}

static int same(const snapshot *a, const snapshot *b) {
  for (int i = 0; i < 5; i++)
    if (a->len[i] != b->len[i] || memcmp(a->data[i], b->data[i], a->len[i]) != 0) return 0;
  return 1;
}

static snapshot g_baseline;
static int g_thread_ok[8];

static void *worker(void *arg) {
  int id = (int)(size_t)arg;
  g_thread_ok[id] = 1;
  for (int round = 0; round < 3; round++) {
    snapshot s;
    memset(&s, 0, sizeof s);
    take_snapshot(&s);
    if (!same(&s, &g_baseline)) g_thread_ok[id] = 0;
    for (int i = 0; i < 5; i++) free(s.data[i]);
  }
  return NULL;
}

int main(int argc, char **argv) {
  CHECK(argc == 3);
  g_fonts = argv[1];
  g_mixed = argv[2];
  CHECK(fontisan_abi_version() == FONTISAN_ABI_VERSION);

  const uint8_t data[] = "hello";
  fontisan_request req = make(FONTISAN_OP_ECHO, data, 5);
  fontisan_result *res = NULL;
  CHECK(fontisan_call(&req, &res) == FONTISAN_OK);
  CHECK(res && res->payload_len == 5 && memcmp(res->payload, "hello", 5) == 0);
  CHECK(res->header.magic == FONTISAN_MAGIC && res->header.struct_size == sizeof *res);
  fontisan_result_free(res);

  /* Stale header: older ABI version must fail loudly with a message, on the new ops too. */
  req = make(FONTISAN_OP_ECHO, data, 5);
  req.header.abi_version = FONTISAN_ABI_VERSION - 1;
  res = NULL;
  CHECK(fontisan_call(&req, &res) == FONTISAN_E_ABI_MISMATCH);
  CHECK(res && res->status == FONTISAN_E_ABI_MISMATCH && res->error.message_len > 0);
  printf("stale header rejected: %.*s\n", (int)res->error.message_len, (const char *)res->error.message);
  fontisan_result_free(res);
  req = make(FONTISAN_OP_READ, data, 5);
  req.header.abi_version = FONTISAN_ABI_VERSION + 1;
  CHECK(fontisan_call(&req, &res) == FONTISAN_E_ABI_MISMATCH);
  fontisan_result_free(res);

  /* A caller compiled against a smaller struct is rejected, not read past. */
  req = make(FONTISAN_OP_ECHO, data, 5);
  req.header.struct_size -= 8;
  CHECK(fontisan_call(&req, &res) == FONTISAN_E_BAD_STRUCT_SIZE);
  fontisan_result_free(res);

  size_t n, an;
  int32_t st;
  uint8_t *ttf = slurp(g_fonts, "TestTTF.ttf", &n);

  /* read: one face; values from the read golden of TestTTF.ttf */
  res = run(FONTISAN_OP_READ, 0, ttf, n, &st);
  CHECK(st == FONTISAN_OK && rows_in(res) == 1);
  CHECK(contains(res->payload, res->payload_len, "\"format\":\"ttf\""));
  CHECK(contains(res->payload, res->payload_len, "\"units_per_em\":1000"));
  CHECK(contains(res->payload, res->payload_len, "\"mode\":\"full\",\"lazy\":false"));
  fontisan_result_free(res);
  res = run(FONTISAN_OP_READ, FONTISAN_READ_METADATA | FONTISAN_READ_LAZY, ttf, n, &st);
  CHECK(st == FONTISAN_OK && contains(res->payload, res->payload_len, "\"mode\":\"metadata\",\"lazy\":true"));
  CHECK(contains(res->payload, res->payload_len, "\"cmap\":null"));
  fontisan_result_free(res);

  /* audit: default has no summary, validate has one */
  uint8_t *ain = audit_input("TestTTF.ttf", ttf, n, &an);
  res = run(FONTISAN_OP_AUDIT, 0, ain, an, &st);
  CHECK(st == FONTISAN_OK && rows_in(res) == 1);
  CHECK(contains(res->payload, res->payload_len, "\"source_file\":\"TestTTF.ttf\""));
  CHECK(contains(res->payload, res->payload_len, "\"postscript_name\":\"TestTTF-Regular\""));
  CHECK(!contains(res->payload, res->payload_len, "validation_summary"));
  fontisan_result_free(res);
  res = run(FONTISAN_OP_AUDIT, FONTISAN_AUDIT_VALIDATE, ain, an, &st);
  CHECK(st == FONTISAN_OK && contains(res->payload, res->payload_len, "\"validation_summary\":{"));
  fontisan_result_free(res);
  free(ain);

  /* malformed (0e truncated-100): the Ruby error surface, read and audit */
  res = run(FONTISAN_OP_READ, 0, ttf, 100, &st);
  CHECK(st == FONTISAN_E_READ_FAILED && res->payload_len == 0);
  CHECK(contains(res->error.message, res->error.message_len, "\"class\":\"RuntimeError\""));
  CHECK(contains(res->error.message, res->error.message_len, "Invalid TrueTypeFont file: End of file reached"));
  CHECK(contains(res->error.message, res->error.message_len, "\"causes\":[{\"class\":\"EOFError\""));
  fontisan_result_free(res);
  ain = audit_input("TestTTF.ttf", ttf, 100, &an);
  res = run(FONTISAN_OP_AUDIT, 0, ain, an, &st);
  CHECK(st == FONTISAN_E_READ_FAILED);
  CHECK(contains(res->error.message, res->error.message_len, "\"class\":\"Fontisan::Error\""));
  CHECK(contains(res->error.message, res->error.message_len,
                 "Failed to load font: Invalid TrueTypeFont file: End of file reached"));
  fontisan_result_free(res);
  free(ain);

  /* dfont: unsupported, for the Ruby engine to serve */
  size_t dn;
  uint8_t *dfont = slurp(g_fonts, "TestDFONT.dfont", &dn);
  res = run(FONTISAN_OP_READ, 0, dfont, dn, &st);
  CHECK(st == FONTISAN_E_UNSUPPORTED);
  fontisan_result_free(res);
  free(dfont);

  /* scan + audit_dir over a mixed directory, one call each */
  res = run(FONTISAN_OP_SCAN, 0, (const uint8_t *)g_mixed, strlen(g_mixed), &st);
  CHECK(st == FONTISAN_OK && rows_in(res) == 5); /* ttf, ttc x2, dfont, pfb */
  CHECK(contains(res->payload, res->payload_len, "\"status\":\"unsupported\""));
  CHECK(contains(res->payload, res->payload_len, "\"family_name\":\"Test TTF\""));
  fontisan_result_free(res);
  res = run(FONTISAN_OP_AUDIT_DIR, 0, (const uint8_t *)g_mixed, strlen(g_mixed), &st);
  CHECK(st == FONTISAN_OK && rows_in(res) == 4);
  CHECK(contains(res->payload, res->payload_len, "\"status\":\"unsupported\""));
  CHECK(contains(res->payload, res->payload_len, "\"status\":\"ok\",\"reports\":["));
  fontisan_result_free(res);
  free(ttf);

  /* threads: concurrent callers get byte-identical payloads */
  memset(&g_baseline, 0, sizeof g_baseline);
  take_snapshot(&g_baseline);
  pthread_t th[8];
  for (size_t i = 0; i < 8; i++) CHECK(pthread_create(&th[i], NULL, worker, (void *)i) == 0);
  for (size_t i = 0; i < 8; i++) { pthread_join(th[i], NULL); CHECK(g_thread_ok[i]); }
  for (int i = 0; i < 5; i++) free(g_baseline.data[i]);

  fontisan_result_free(NULL);
  puts("c abi test ok");
  return 0;
}
