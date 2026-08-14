/* Load a Tangent.vst3 bundle the way a DAW does, and ask it what it is.
 *
 *   cc -o /tmp/verify-plugin scripts/verify-plugin.c && /tmp/verify-plugin <bundle>
 *
 * WHY THIS IS C AND NOT A CARGO TEST
 *
 * The one thing a `cargo build` cannot tell you about a plugin is whether a
 * host can load it. Everything between a compiling crate and a working plugin
 * — the bundle layout, the Info.plist package type, the exported entry points,
 * the code signature, the factory's own view of itself — lives outside the
 * compiler, and every one of them fails silently: a DAW that cannot load a
 * plugin does not say why, it just does not list it.
 *
 * So this does exactly what a VST3 host does, in order: dlopen the bundle
 * binary, call `bundleEntry`, call `GetPluginFactory`, walk the factory's COM
 * vtable for the vendor block and every class it advertises, then CREATE an
 * instance and initialise it. That last step matters on its own — a plugin can
 * advertise itself perfectly and still panic the host the moment it is added
 * to a track, because `createInstance` is what first runs
 * `Default::default()`, and Tangent's reads a settings file there.
 *
 * The vtable layouts below are the VST3 SDK's `IPluginFactory`, `IComponent`
 * and their info structs, which are ABI-frozen: they are what makes a .vst3
 * built by any toolchain loadable by any host. Nothing here is processed and
 * no window is opened; the instance is terminated and released.
 */
#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef int32_t tresult;
#define kResultOk 0

typedef struct {
    char vendor[64];
    char url[256];
    char email[128];
    int32_t flags;
} PFactoryInfo;

typedef struct {
    char cid[16];
    int32_t cardinality;
    char category[32];
    char name[64];
} PClassInfo;

struct IPluginFactory;

typedef struct {
    /* FUnknown */
    tresult (*queryInterface)(struct IPluginFactory *, const char *, void **);
    uint32_t (*addRef)(struct IPluginFactory *);
    uint32_t (*release)(struct IPluginFactory *);
    /* IPluginFactory */
    tresult (*getFactoryInfo)(struct IPluginFactory *, PFactoryInfo *);
    int32_t (*countClasses)(struct IPluginFactory *);
    tresult (*getClassInfo)(struct IPluginFactory *, int32_t, PClassInfo *);
    tresult (*createInstance)(struct IPluginFactory *, const char *, const char *, void **);
} IPluginFactoryVtbl;

struct IPluginFactory {
    const IPluginFactoryVtbl *vtbl;
};

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s <path to binary inside the .vst3 bundle>\n", argv[0]);
        return 2;
    }

    void *lib = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!lib) {
        fprintf(stderr, "FAIL: the host could not load the binary: %s\n", dlerror());
        return 1;
    }
    printf("  loaded            ok\n");

#ifdef __APPLE__
    /* macOS hosts call this before anything else; nih-plug installs its logger
     * here. A plugin that skips it is not initialised the way a DAW expects. */
    int (*entry)(void *) = (int (*)(void *))dlsym(lib, "bundleEntry");
    if (!entry) {
        fprintf(stderr, "FAIL: no bundleEntry — macOS hosts will not load this\n");
        return 1;
    }
    if (!entry(NULL)) {
        fprintf(stderr, "FAIL: bundleEntry returned false\n");
        return 1;
    }
    printf("  bundleEntry       ok\n");
#endif

    struct IPluginFactory *(*get_factory)(void) =
        (struct IPluginFactory * (*)(void)) dlsym(lib, "GetPluginFactory");
    if (!get_factory) {
        fprintf(stderr, "FAIL: no GetPluginFactory — this is not a VST3\n");
        return 1;
    }
    struct IPluginFactory *factory = get_factory();
    if (!factory) {
        fprintf(stderr, "FAIL: GetPluginFactory returned null\n");
        return 1;
    }
    printf("  GetPluginFactory  ok\n");

    PFactoryInfo info;
    memset(&info, 0, sizeof info);
    if (factory->vtbl->getFactoryInfo(factory, &info) != kResultOk) {
        fprintf(stderr, "FAIL: getFactoryInfo failed\n");
        return 1;
    }
    printf("  vendor            %s\n", info.vendor);
    printf("  url               %s\n", info.url);
    printf("  email             %s\n", info.email);

    int32_t n = factory->vtbl->countClasses(factory);
    printf("  classes           %d\n", n);
    if (n < 1) {
        fprintf(stderr, "FAIL: the factory advertises no plugin\n");
        return 1;
    }

    int found_audio_module = 0;
    for (int32_t i = 0; i < n; i++) {
        PClassInfo c;
        memset(&c, 0, sizeof c);
        if (factory->vtbl->getClassInfo(factory, i, &c) != kResultOk) {
            fprintf(stderr, "FAIL: getClassInfo(%d) failed\n", i);
            return 1;
        }
        printf("    [%d] name       %s\n", i, c.name);
        printf("        category   %s\n", c.category);
        printf("        cid        ");
        for (int k = 0; k < 16; k++) printf("%02X", (unsigned char)c.cid[k]);
        printf("\n");
        if (strcmp(c.category, "Audio Module Class") == 0) found_audio_module = 1;
    }

    if (!found_audio_module) {
        fprintf(stderr, "FAIL: no 'Audio Module Class' — a host would list nothing\n");
        return 1;
    }

    /* Instantiate it, which is the step that runs `Default::default()` and
     * `initialize()`. A plugin can advertise itself perfectly and then panic
     * the host the moment it is added to a track; loading is not enough.
     *
     * The IID below is `IComponent`'s, frozen by the VST3 SDK. On every
     * platform but Windows the TUID is the GUID's bytes in written order. */
    PClassInfo c0;
    memset(&c0, 0, sizeof c0);
    factory->vtbl->getClassInfo(factory, 0, &c0);
    static const char kIComponentIID[16] = {
        (char)0xE8, (char)0x31, (char)0xFF, (char)0x31,
        (char)0xF2, (char)0xD5, (char)0x43, (char)0x01,
        (char)0x92, (char)0x8E, (char)0xBB, (char)0xEE,
        (char)0x25, (char)0x69, (char)0x78, (char)0x02,
    };

    struct IComponent;
    typedef struct {
        tresult (*queryInterface)(struct IComponent *, const char *, void **);
        uint32_t (*addRef)(struct IComponent *);
        uint32_t (*release)(struct IComponent *);
        /* IPluginBase */
        tresult (*initialize)(struct IComponent *, void *context);
        tresult (*terminate)(struct IComponent *);
        /* IComponent */
        tresult (*getControllerClassId)(struct IComponent *, char *);
        tresult (*setIoMode)(struct IComponent *, int32_t);
        int32_t (*getBusCount)(struct IComponent *, int32_t type, int32_t dir);
    } IComponentVtbl;
    struct IComponent { const IComponentVtbl *vtbl; };

    struct IComponent *comp = NULL;
    if (factory->vtbl->createInstance(factory, c0.cid, kIComponentIID, (void **)&comp)
            != kResultOk || !comp) {
        fprintf(stderr, "FAIL: createInstance did not give back an IComponent\n");
        return 1;
    }
    printf("  createInstance    ok\n");

    if (comp->vtbl->initialize(comp, NULL) != kResultOk) {
        fprintf(stderr, "FAIL: initialize() refused\n");
        return 1;
    }
    printf("  initialize        ok\n");

    /* kAudio = 0, kEvent = 1; kInput = 0, kOutput = 1. Tangent declares one
     * event input and no audio at all, and a host reads exactly this to decide
     * what it can route into the plugin. */
    int32_t ev_in  = comp->vtbl->getBusCount(comp, 1, 0);
    int32_t au_in  = comp->vtbl->getBusCount(comp, 0, 0);
    int32_t au_out = comp->vtbl->getBusCount(comp, 0, 1);
    printf("  buses             %d event in, %d audio in, %d audio out\n",
           ev_in, au_in, au_out);
    if (ev_in < 1) {
        fprintf(stderr, "FAIL: no event input — no MIDI could reach it\n");
        return 1;
    }
    if (au_in != 0 || au_out != 0) {
        fprintf(stderr, "FAIL: it claims audio buses it does not use\n");
        return 1;
    }

    comp->vtbl->terminate(comp);
    comp->vtbl->release(comp);
    factory->vtbl->release(factory);
    printf("OK: a host can load this bundle, list it, and instantiate it.\n");
    return 0;
}
