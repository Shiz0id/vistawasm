/*
  * hlslc: compile a list of HLSL files with Microsoft's D3DCompiler DLL.
  *
  *   hlslc.exe COMPILER_DLL LIST [OUT_DIR]
  *
  * LIST has one shader a line: "<file> <entry point> <profile>". Each file is
  * compiled as fxc /O3 would compile it. Warnings are printed, not fatal. With
  * OUT_DIR, each result is written as <OUT_DIR>/<file with / as __>.cso.
  * Errors and warnings go to stderr; the exit status is the failure count.
  *
  * It runs on Windows and under Wine, which is how check-hlsl.sh uses it on
  * Linux. Built with MinGW: x86_64-w64-mingw32-gcc -O2 -o hlslc.exe hlslc.c
  *
  * Licence: AGPL-3.0-only, as VistaWASM.
  */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

/* The two interfaces D3DCompile hands back, declared here so the tool needs
  * no SDK headers. ID3DBlob's vtable: QueryInterface, AddRef, Release,
  * GetBufferPointer, GetBufferSize. */
typedef struct Blob Blob;
typedef struct BlobVtbl {
  void *query_interface;
  void *add_ref;
  ULONG (__stdcall *release)(Blob *self);
  void *(__stdcall *get_buffer_pointer)(Blob *self);
  SIZE_T (__stdcall *get_buffer_size)(Blob *self);
} BlobVtbl;
struct Blob {
  const BlobVtbl *vtbl;
};

typedef HRESULT (__stdcall *CompileFn)(const void *source, SIZE_T size, const char *name,
                                        const void *defines, void *include, const char *entry,
                                        const char *target, UINT flags1, UINT flags2, Blob **code,
                                        Blob **errors);

/* D3DCOMPILE_OPTIMIZATION_LEVEL3 */
#define COMPILE_FLAGS (1u << 15)

static char *read_file(const char *path, size_t *size) {
  FILE *file = fopen(path, "rb");

  if (file == NULL) {
    return NULL;
  }

  fseek(file, 0, SEEK_END);
  long length = ftell(file);
  fseek(file, 0, SEEK_SET);
  char *text = malloc((size_t)length + 1);

  if (text == NULL || fread(text, 1, (size_t)length, file) != (size_t)length) {
    fclose(file);
    free(text);
    return NULL;
  }

  fclose(file);
  text[length] = '\0';
  *size = (size_t)length;
  return text;
}

int main(int argc, char **argv) {
  if (argc < 3) {
    fprintf(stderr, "usage: hlslc.exe COMPILER_DLL LIST [OUT_DIR]\n");
    return 2;
  }

  HMODULE dll = LoadLibraryA(argv[1]);

  if (dll == NULL) {
    fprintf(stderr, "could not load %s (error %lu)\n", argv[1], GetLastError());
    return 2;
  }

  CompileFn compile = (CompileFn)(void *)GetProcAddress(dll, "D3DCompile");

  if (compile == NULL) {
    fprintf(stderr, "%s has no D3DCompile\n", argv[1]);
    return 2;
  }

  FILE *list = fopen(argv[2], "r");

  if (list == NULL) {
    fprintf(stderr, "could not open %s\n", argv[2]);
    return 2;
  }

  char line[1024];
  int failures = 0, total = 0;

  while (fgets(line, sizeof(line), list) != NULL) {
    char path[512], entry[128], profile[32];

    if (sscanf(line, "%511s %127s %31s", path, entry, profile) != 3) {
      continue;
    }

    total += 1;
    size_t size = 0;
    char *source = read_file(path, &size);

    if (source == NULL) {
      fprintf(stderr, "%s: could not read\n", path);
      failures += 1;
      continue;
    }

    Blob *code = NULL, *errors = NULL;
    ULONGLONG started = GetTickCount64();
    HRESULT result = compile(source, size, path, NULL, NULL, entry, profile, COMPILE_FLAGS, 0, &code, &errors);
    free(source);
    printf("%7.1f s  %s\n", (double)(GetTickCount64() - started) / 1000.0, path);
    fflush(stdout);

    if (errors != NULL) {
      fprintf(stderr, "%s (%s %s):\n%.*s\n", path, profile, entry, (int)errors->vtbl->get_buffer_size(errors),
              (const char *)errors->vtbl->get_buffer_pointer(errors));
      errors->vtbl->release(errors);
    }

    if (FAILED(result) || code == NULL) {
      fprintf(stderr, "FAILED %s (0x%08lx)\n", path, (unsigned long)result);
      failures += 1;

      /* An older compile of the file must not pass for this one. */
      if (argc > 3) {
        const char *relative = strncmp(path, "hlsl/", 5) == 0 ? path + 5 : path;
        char stale[1024];
        snprintf(stale, sizeof(stale), "%s/%s", argv[3], relative);
        size_t at = strlen(stale);

        if (at > 5 && strcmp(stale + at - 5, ".hlsl") == 0) {
          strcpy(stale + at - 5, ".cso");
          DeleteFileA(stale);
        }
      }

      continue;
    }

    if (argc > 3) {
      /* hlsl/<module>/<entry>.hlsl -> OUT_DIR/<module>/<entry>.cso */
      const char *relative = strncmp(path, "hlsl/", 5) == 0 ? path + 5 : path;
      char out[1024];
      snprintf(out, sizeof(out), "%s/%s", argv[3], relative);
      size_t at = strlen(out);

      if (at > 5 && strcmp(out + at - 5, ".hlsl") == 0) {
        strcpy(out + at - 5, ".cso");
      }

      char *slash = strrchr(out, '/');

      if (slash != NULL) {
        *slash = '\0';
        CreateDirectoryA(out, NULL);
        *slash = '/';
      }

      FILE *file = fopen(out, "wb");

      if (file == NULL) {
        fprintf(stderr, "%s: could not write %s\n", path, out);
        failures += 1;
      } else {
        fwrite(code->vtbl->get_buffer_pointer(code), 1, code->vtbl->get_buffer_size(code), file);
        fclose(file);
      }
    }

    code->vtbl->release(code);
  }

  fclose(list);
  printf("%d of %d shaders compiled\n", total - failures, total);
  return failures;
}
