#include <dlfcn.h>
#include <limits.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifndef DICTA_QIANWEN_DEFAULT_UNET_RUNTIME_PATH
#define DICTA_QIANWEN_DEFAULT_UNET_RUNTIME_PATH "libqianwen_unet_runtime.dylib"
#endif

#define DICTA_QIANWEN_WSG_APP_KEY 15350
#define DICTA_QIANWEN_WSG_SIGN_CAPACITY 1024

static int instance_value = 0x51575347;
static pthread_once_t unet_once = PTHREAD_ONCE_INIT;
static void *unet_library = NULL;
static int unet_ready = 0;

typedef int (*qwen_unet_init_fn)(const char *, const char *);
typedef char *(*qwen_unet_string_fn)(const char *);
typedef void (*qwen_unet_free_fn)(char *);

static qwen_unet_string_fn unet_sign = NULL;
static qwen_unet_string_fn unet_encrypt_base64 = NULL;
static qwen_unet_string_fn unet_decrypt_base64 = NULL;
static qwen_unet_free_fn unet_free = NULL;

static void debug_log(const char *message) {
    const char *enabled = getenv("DICTA_QIANWEN_WSG_SHIM_DEBUG");
    if (!enabled || !*enabled) {
        return;
    }
    fprintf(stderr, "[dicta-qianwen-wsg-shim] %s\n", message);
}

static void init_unet_once(void) {
    const char *path = getenv("DICTA_QIANWEN_UNET_RUNTIME_PATH");
    if (!path || !*path) {
        path = DICTA_QIANWEN_DEFAULT_UNET_RUNTIME_PATH;
    }

    unet_library = dlopen(path, RTLD_NOW | RTLD_GLOBAL);
    if (!unet_library) {
        debug_log(dlerror());
        return;
    }

    qwen_unet_init_fn init =
        (qwen_unet_init_fn)dlsym(unet_library, "qianwen_unet_initialize_for_process");
    unet_sign =
        (qwen_unet_string_fn)dlsym(unet_library, "qianwen_unet_sign_with_internal_wsg");
    unet_encrypt_base64 =
        (qwen_unet_string_fn)dlsym(unet_library, "qianwen_unet_encrypt_with_internal_wsg_base64");
    unet_decrypt_base64 =
        (qwen_unet_string_fn)dlsym(unet_library, "qianwen_unet_decrypt_base64_with_internal_wsg");
    unet_free = (qwen_unet_free_fn)dlsym(unet_library, "qianwen_unet_string_free");

    if (!init || !unet_sign || !unet_encrypt_base64 || !unet_decrypt_base64 || !unet_free) {
        debug_log("missing qianwen_unet_runtime symbol");
        return;
    }

    const char *process_name = getenv("DICTA_QIANWEN_UNET_PROCESS_NAME");
    if (!process_name || !*process_name) {
        process_name = "qianwen-ime";
    }
    const char *sdk_dir = getenv("QWEN_SHELL_UTDID_SDK_DIR");
    if (!sdk_dir) {
        sdk_dir = "";
    }

    unet_ready = init(process_name, sdk_dir) != 0;
}

static int ensure_unet(void) {
    pthread_once(&unet_once, init_unet_once);
    return unet_ready;
}

static char *copy_input(const char *input, size_t input_len) {
    if (!input) {
        return NULL;
    }

    char *owned = (char *)malloc(input_len + 1);
    if (!owned) {
        return NULL;
    }
    memcpy(owned, input, input_len);
    owned[input_len] = 0;
    return owned;
}

static char *call_unet_string(qwen_unet_string_fn fn, const char *input, size_t input_len) {
    if (!ensure_unet() || !fn) {
        return NULL;
    }

    char *owned = copy_input(input, input_len);
    if (!owned) {
        return NULL;
    }
    char *result = fn(owned);
    free(owned);
    return result;
}

static int copy_output(char *output, size_t output_capacity, const char *value) {
    if (!output || !value) {
        return -1;
    }

    size_t len = strlen(value);
    if (len == 0 || len > INT_MAX || output_capacity == 0 || len + 1 > output_capacity) {
        return -1;
    }
    memcpy(output, value, len + 1);
    return (int)len;
}

static size_t owned_result_size(char *value) {
    if (!value) {
        return 0;
    }

    size_t len = strlen(value);
    if (unet_free) {
        unet_free(value);
    }
    return len == 0 ? 0 : len + 1;
}

void *WSG_CreateInstance(void) {
    ensure_unet();
    return &instance_value;
}

int WSG_DestroyInstance(void *instance) {
    (void)instance;
    return 0;
}

size_t WSG_GetEncryptedToBase64Size(size_t input_len) {
    return ((input_len + 2) / 3) * 4 + 64;
}

size_t WSG_GetDecryptedFromBase64Size(size_t input_len) {
    return input_len + 64;
}

size_t WSG_GetSignSize(void) {
    return DICTA_QIANWEN_WSG_SIGN_CAPACITY;
}

int WSG_EncryptToBase64(
    void *instance,
    int app_key,
    const char *input,
    size_t input_len,
    char *output,
    size_t output_capacity
) {
    (void)instance;
    (void)app_key;
    char *value = call_unet_string(unet_encrypt_base64, input, input_len);
    int status = copy_output(output, output_capacity, value);
    if (value && unet_free) {
        unet_free(value);
    }
    return status;
}

int WSG_DecryptFromBase64(
    void *instance,
    uint16_t *number,
    const char *input,
    size_t input_len,
    char *output,
    size_t output_capacity
) {
    (void)instance;
    if (number) {
        *number = DICTA_QIANWEN_WSG_APP_KEY;
    }
    char *value = call_unet_string(unet_decrypt_base64, input, input_len);
    int status = copy_output(output, output_capacity, value);
    if (value && unet_free) {
        unet_free(value);
    }
    return status;
}

int WSG_Sign(
    void *instance,
    int app_key,
    const char *input,
    size_t input_len,
    char *output,
    size_t output_capacity
) {
    (void)instance;
    (void)app_key;
    char *value = call_unet_string(unet_sign, input, input_len);
    int status = copy_output(output, output_capacity, value);
    if (value && unet_free) {
        unet_free(value);
    }
    return status;
}
