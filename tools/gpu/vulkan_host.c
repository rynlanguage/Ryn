// Runs one SPIR-V compute entry point on the first Vulkan device, for the GPU tests of the Ryn SPIR-V backend.
// usage: vulkan_host <module.spv> <spec.txt>   (Windows: vulkan-1.dll; Linux: libvulkan.so.1)
// The spec file has lines:  entry NAME  |  groups N  |  buffer BINDING TYPE COUNT [iota | v0 v1 ...]
// TYPE is u32, i32, u64 or f32. The result of each buffer is printed as: buffer BINDING: v0 v1 ...
#define VK_NO_PROTOTYPES
#include <vulkan/vulkan.h>
#ifdef _WIN32
#include <windows.h>
#else
#include <dlfcn.h>
#endif
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

#define MAX_BUFFERS 8

typedef struct {
    int binding;
    char type[8];
    uint32_t count;
    uint32_t size;
    VkBuffer buffer;
    VkDeviceMemory memory;
    void *mapped;
    char init[4096];
    int iota;
} Buffer;

static PFN_vkGetInstanceProcAddr get_instance_proc;
static VkInstance instance;
static VkDevice device;
static VkPhysicalDevice physical;

static void fail(const char *what, VkResult result) {
    fprintf(stderr, "error: %s (VkResult %d)\n", what, (int)result);
    exit(2);
}

static void *load_fn(const char *name) {
    void *fn = (void *)get_instance_proc(instance, name);
    if (!fn) {
        fprintf(stderr, "error: missing Vulkan function %s\n", name);
        exit(2);
    }
    return fn;
}

static uint32_t memory_type(VkPhysicalDeviceMemoryProperties *props, uint32_t bits, VkMemoryPropertyFlags want) {
    for (uint32_t i = 0; i < props->memoryTypeCount; i++) {
        if ((bits & (1u << i)) && (props->memoryTypes[i].propertyFlags & want) == want) return i;
    }
    fprintf(stderr, "error: no memory type with the required flags\n");
    exit(2);
}

static uint32_t element_size(const char *type) {
    if (strcmp(type, "u64") == 0) return 8;
    return 4;
}

static void write_value(Buffer *b, uint32_t index, const char *text) {
    uint8_t *base = (uint8_t *)b->mapped;
    if (strcmp(b->type, "f32") == 0) {
        float v = strtof(text, NULL);
        memcpy(base + index * 4, &v, 4);
    } else if (strcmp(b->type, "u64") == 0) {
        uint64_t v = strtoull(text, NULL, 10);
        memcpy(base + index * 8, &v, 8);
    } else if (strcmp(b->type, "i32") == 0) {
        int32_t v = (int32_t)strtol(text, NULL, 10);
        memcpy(base + index * 4, &v, 4);
    } else {
        uint32_t v = (uint32_t)strtoul(text, NULL, 10);
        memcpy(base + index * 4, &v, 4);
    }
}

static void print_value(Buffer *b, uint32_t index) {
    uint8_t *base = (uint8_t *)b->mapped;
    if (strcmp(b->type, "f32") == 0) {
        float v;
        memcpy(&v, base + index * 4, 4);
        printf("%.9g", (double)v);
    } else if (strcmp(b->type, "u64") == 0) {
        uint64_t v;
        memcpy(&v, base + index * 8, 8);
        printf("%llu", (unsigned long long)v);
    } else if (strcmp(b->type, "i32") == 0) {
        int32_t v;
        memcpy(&v, base + index * 4, 4);
        printf("%d", (int)v);
    } else {
        uint32_t v;
        memcpy(&v, base + index * 4, 4);
        printf("%u", (unsigned)v);
    }
}

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: host <module.spv> <spec.txt>\n");
        return 2;
    }
    FILE *spv = fopen(argv[1], "rb");
    if (!spv) { fprintf(stderr, "error: cannot read %s\n", argv[1]); return 2; }
    fseek(spv, 0, SEEK_END);
    long spv_size = ftell(spv);
    fseek(spv, 0, SEEK_SET);
    uint32_t *code = (uint32_t *)malloc((size_t)spv_size);
    if (fread(code, 1, (size_t)spv_size, spv) != (size_t)spv_size) { fprintf(stderr, "error: short read\n"); return 2; }
    fclose(spv);

    char entry[256] = "main";
    uint32_t groups = 1;
    Buffer buffers[MAX_BUFFERS];
    int buffer_count = 0;
    FILE *spec = fopen(argv[2], "r");
    if (!spec) { fprintf(stderr, "error: cannot read %s\n", argv[2]); return 2; }
    char line[8192];
    while (fgets(line, sizeof line, spec)) {
        char *tok = strtok(line, " \t\r\n");
        if (!tok || tok[0] == '#') continue;
        if (strcmp(tok, "entry") == 0) {
            strncpy(entry, strtok(NULL, " \t\r\n"), sizeof entry - 1);
        } else if (strcmp(tok, "groups") == 0) {
            groups = (uint32_t)strtoul(strtok(NULL, " \t\r\n"), NULL, 10);
        } else if (strcmp(tok, "buffer") == 0) {
            if (buffer_count >= MAX_BUFFERS) { fprintf(stderr, "error: too many buffers\n"); return 2; }
            Buffer *b = &buffers[buffer_count++];
            memset(b, 0, sizeof *b);
            b->binding = (int)strtol(strtok(NULL, " \t\r\n"), NULL, 10);
            strncpy(b->type, strtok(NULL, " \t\r\n"), sizeof b->type - 1);
            b->count = (uint32_t)strtoul(strtok(NULL, " \t\r\n"), NULL, 10);
            b->size = b->count * element_size(b->type);
            char *values = strtok(NULL, "\r\n");
            while (values && (*values == ' ' || *values == '\t')) values++;
            b->iota = values && strncmp(values, "iota", 4) == 0;
            if (values && !b->iota) {
                strncpy(b->init, values, sizeof b->init - 1);
            }
        }
    }
    fclose(spec);

#ifdef _WIN32
    HMODULE vulkan = LoadLibraryA("vulkan-1.dll");
    if (!vulkan) { fprintf(stderr, "error: vulkan-1.dll not found\n"); return 2; }
    get_instance_proc = (PFN_vkGetInstanceProcAddr)GetProcAddress(vulkan, "vkGetInstanceProcAddr");
#else
    void *vulkan = dlopen("libvulkan.so.1", RTLD_NOW);
    if (!vulkan) { fprintf(stderr, "error: libvulkan.so.1 not found\n"); return 2; }
    get_instance_proc = (PFN_vkGetInstanceProcAddr)dlsym(vulkan, "vkGetInstanceProcAddr");
#endif

    VkApplicationInfo app = { VK_STRUCTURE_TYPE_APPLICATION_INFO };
    app.pApplicationName = "ryn-spirv-test";
    app.apiVersion = VK_API_VERSION_1_1;
    VkInstanceCreateInfo ici = { VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO };
    ici.pApplicationInfo = &app;
    PFN_vkCreateInstance create_instance = (PFN_vkCreateInstance)get_instance_proc(NULL, "vkCreateInstance");
    VkResult r = create_instance(&ici, NULL, &instance);
    if (r != VK_SUCCESS) fail("vkCreateInstance", r);

    PFN_vkEnumeratePhysicalDevices enumerate = (PFN_vkEnumeratePhysicalDevices)load_fn("vkEnumeratePhysicalDevices");
    uint32_t device_count = 0;
    enumerate(instance, &device_count, NULL);
    if (device_count == 0) { fprintf(stderr, "error: no Vulkan device\n"); return 2; }
    VkPhysicalDevice devices[8];
    if (device_count > 8) device_count = 8;
    enumerate(instance, &device_count, devices);
    physical = devices[0];

    PFN_vkGetPhysicalDeviceQueueFamilyProperties queue_props = (PFN_vkGetPhysicalDeviceQueueFamilyProperties)load_fn("vkGetPhysicalDeviceQueueFamilyProperties");
    uint32_t family_count = 0;
    queue_props(physical, &family_count, NULL);
    VkQueueFamilyProperties families[32];
    if (family_count > 32) family_count = 32;
    queue_props(physical, &family_count, families);
    uint32_t family = UINT32_MAX;
    for (uint32_t i = 0; i < family_count; i++) {
        if (families[i].queueFlags & VK_QUEUE_COMPUTE_BIT) { family = i; break; }
    }
    if (family == UINT32_MAX) { fprintf(stderr, "error: no compute queue\n"); return 2; }

    float priority = 1.0f;
    VkDeviceQueueCreateInfo qci = { VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO };
    qci.queueFamilyIndex = family;
    qci.queueCount = 1;
    qci.pQueuePriorities = &priority;
    VkDeviceCreateInfo dci = { VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO };
    dci.queueCreateInfoCount = 1;
    dci.pQueueCreateInfos = &qci;
    PFN_vkCreateDevice create_device = (PFN_vkCreateDevice)load_fn("vkCreateDevice");
    r = create_device(physical, &dci, NULL, &device);
    if (r != VK_SUCCESS) fail("vkCreateDevice", r);

    PFN_vkGetDeviceQueue get_queue = (PFN_vkGetDeviceQueue)load_fn("vkGetDeviceQueue");
    VkQueue queue;
    get_queue(device, family, 0, &queue);

    PFN_vkGetPhysicalDeviceMemoryProperties mem_props_fn = (PFN_vkGetPhysicalDeviceMemoryProperties)load_fn("vkGetPhysicalDeviceMemoryProperties");
    VkPhysicalDeviceMemoryProperties mem_props;
    mem_props_fn(physical, &mem_props);
    PFN_vkCreateBuffer create_buffer = (PFN_vkCreateBuffer)load_fn("vkCreateBuffer");
    PFN_vkGetBufferMemoryRequirements buffer_reqs = (PFN_vkGetBufferMemoryRequirements)load_fn("vkGetBufferMemoryRequirements");
    PFN_vkAllocateMemory allocate = (PFN_vkAllocateMemory)load_fn("vkAllocateMemory");
    PFN_vkBindBufferMemory bind = (PFN_vkBindBufferMemory)load_fn("vkBindBufferMemory");
    PFN_vkMapMemory map = (PFN_vkMapMemory)load_fn("vkMapMemory");

    for (int i = 0; i < buffer_count; i++) {
        Buffer *b = &buffers[i];
        VkBufferCreateInfo bci = { VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO };
        bci.size = b->size;
        bci.usage = VK_BUFFER_USAGE_STORAGE_BUFFER_BIT;
        bci.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
        r = create_buffer(device, &bci, NULL, &b->buffer);
        if (r != VK_SUCCESS) fail("vkCreateBuffer", r);
        VkMemoryRequirements req;
        buffer_reqs(device, b->buffer, &req);
        VkMemoryAllocateInfo mai = { VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO };
        mai.allocationSize = req.size;
        mai.memoryTypeIndex = memory_type(&mem_props, req.memoryTypeBits, VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT);
        r = allocate(device, &mai, NULL, &b->memory);
        if (r != VK_SUCCESS) fail("vkAllocateMemory", r);
        bind(device, b->buffer, b->memory, 0);
        r = map(device, b->memory, 0, VK_WHOLE_SIZE, 0, &b->mapped);
        if (r != VK_SUCCESS) fail("vkMapMemory", r);
        if (b->size > 0) memset(b->mapped, 0, b->size);
        if (b->iota) {
            for (uint32_t k = 0; k < b->count; k++) { char t[32]; sprintf(t, "%u", k); write_value(b, k, t); }
        } else if (b->init[0]) {
            char *v = strtok(b->init, " \t\r\n");
            uint32_t k = 0;
            while (v && k < b->count) { write_value(b, k, v); v = strtok(NULL, " \t\r\n"); k++; }
        }
    }

    // Descriptor set layout: one storage buffer per binding, all visible to the compute stage.
    VkDescriptorSetLayoutBinding dsb[MAX_BUFFERS];
    for (int i = 0; i < buffer_count; i++) {
        dsb[i].binding = (uint32_t)buffers[i].binding;
        dsb[i].descriptorType = VK_DESCRIPTOR_TYPE_STORAGE_BUFFER;
        dsb[i].descriptorCount = 1;
        dsb[i].stageFlags = VK_SHADER_STAGE_COMPUTE_BIT;
        dsb[i].pImmutableSamplers = NULL;
    }
    VkDescriptorSetLayoutCreateInfo dslci = { VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO };
    dslci.bindingCount = (uint32_t)buffer_count;
    dslci.pBindings = dsb;
    VkDescriptorSetLayout set_layout;
    r = ((PFN_vkCreateDescriptorSetLayout)load_fn("vkCreateDescriptorSetLayout"))(device, &dslci, NULL, &set_layout);
    if (r != VK_SUCCESS) fail("vkCreateDescriptorSetLayout", r);
    VkPipelineLayoutCreateInfo plci = { VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO };
    plci.setLayoutCount = 1;
    plci.pSetLayouts = &set_layout;
    VkPipelineLayout pipeline_layout;
    r = ((PFN_vkCreatePipelineLayout)load_fn("vkCreatePipelineLayout"))(device, &plci, NULL, &pipeline_layout);
    if (r != VK_SUCCESS) fail("vkCreatePipelineLayout", r);

    VkDescriptorPoolSize pool_size = { VK_DESCRIPTOR_TYPE_STORAGE_BUFFER, (uint32_t)buffer_count };
    VkDescriptorPoolCreateInfo dpci = { VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO };
    dpci.maxSets = 1;
    dpci.poolSizeCount = 1;
    dpci.pPoolSizes = &pool_size;
    VkDescriptorPool pool;
    r = ((PFN_vkCreateDescriptorPool)load_fn("vkCreateDescriptorPool"))(device, &dpci, NULL, &pool);
    if (r != VK_SUCCESS) fail("vkCreateDescriptorPool", r);
    VkDescriptorSetAllocateInfo dsai = { VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO };
    dsai.descriptorPool = pool;
    dsai.descriptorSetCount = 1;
    dsai.pSetLayouts = &set_layout;
    VkDescriptorSet set;
    r = ((PFN_vkAllocateDescriptorSets)load_fn("vkAllocateDescriptorSets"))(device, &dsai, &set);
    if (r != VK_SUCCESS) fail("vkAllocateDescriptorSets", r);
    VkDescriptorBufferInfo infos[MAX_BUFFERS];
    VkWriteDescriptorSet writes[MAX_BUFFERS];
    for (int i = 0; i < buffer_count; i++) {
        infos[i].buffer = buffers[i].buffer;
        infos[i].offset = 0;
        infos[i].range = VK_WHOLE_SIZE;
        writes[i].sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
        writes[i].pNext = NULL;
        writes[i].dstSet = set;
        writes[i].dstBinding = (uint32_t)buffers[i].binding;
        writes[i].dstArrayElement = 0;
        writes[i].descriptorCount = 1;
        writes[i].descriptorType = VK_DESCRIPTOR_TYPE_STORAGE_BUFFER;
        writes[i].pImageInfo = NULL;
        writes[i].pBufferInfo = &infos[i];
        writes[i].pTexelBufferView = NULL;
    }
    if (buffer_count > 0) ((PFN_vkUpdateDescriptorSets)load_fn("vkUpdateDescriptorSets"))(device, (uint32_t)buffer_count, writes, 0, NULL);

    VkShaderModuleCreateInfo smci = { VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO };
    smci.codeSize = (size_t)spv_size;
    smci.pCode = code;
    VkShaderModule module;
    r = ((PFN_vkCreateShaderModule)load_fn("vkCreateShaderModule"))(device, &smci, NULL, &module);
    if (r != VK_SUCCESS) fail("vkCreateShaderModule", r);

    VkPipelineShaderStageCreateInfo stage = { VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO };
    stage.stage = VK_SHADER_STAGE_COMPUTE_BIT;
    stage.module = module;
    stage.pName = entry;
    VkComputePipelineCreateInfo cpci = { VK_STRUCTURE_TYPE_COMPUTE_PIPELINE_CREATE_INFO };
    cpci.stage = stage;
    cpci.layout = pipeline_layout;
    VkPipeline pipeline;
    r = ((PFN_vkCreateComputePipelines)load_fn("vkCreateComputePipelines"))(device, VK_NULL_HANDLE, 1, &cpci, NULL, &pipeline);
    if (r != VK_SUCCESS) fail("vkCreateComputePipelines (the module or its entry point was rejected)", r);

    VkCommandPoolCreateInfo cpi = { VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO };
    cpi.queueFamilyIndex = family;
    VkCommandPool command_pool;
    r = ((PFN_vkCreateCommandPool)load_fn("vkCreateCommandPool"))(device, &cpi, NULL, &command_pool);
    if (r != VK_SUCCESS) fail("vkCreateCommandPool", r);
    VkCommandBufferAllocateInfo cbai = { VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO };
    cbai.commandPool = command_pool;
    cbai.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
    cbai.commandBufferCount = 1;
    VkCommandBuffer cmd;
    r = ((PFN_vkAllocateCommandBuffers)load_fn("vkAllocateCommandBuffers"))(device, &cbai, &cmd);
    if (r != VK_SUCCESS) fail("vkAllocateCommandBuffers", r);
    VkCommandBufferBeginInfo cbbi = { VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO };
    cbbi.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
    ((PFN_vkBeginCommandBuffer)load_fn("vkBeginCommandBuffer"))(cmd, &cbbi);
    ((PFN_vkCmdBindPipeline)load_fn("vkCmdBindPipeline"))(cmd, VK_PIPELINE_BIND_POINT_COMPUTE, pipeline);
    ((PFN_vkCmdBindDescriptorSets)load_fn("vkCmdBindDescriptorSets"))(cmd, VK_PIPELINE_BIND_POINT_COMPUTE, pipeline_layout, 0, 1, &set, 0, NULL);
    ((PFN_vkCmdDispatch)load_fn("vkCmdDispatch"))(cmd, groups, 1, 1);
    ((PFN_vkEndCommandBuffer)load_fn("vkEndCommandBuffer"))(cmd);

    VkFenceCreateInfo fci = { VK_STRUCTURE_TYPE_FENCE_CREATE_INFO };
    VkFence fence;
    ((PFN_vkCreateFence)load_fn("vkCreateFence"))(device, &fci, NULL, &fence);
    VkSubmitInfo si = { VK_STRUCTURE_TYPE_SUBMIT_INFO };
    si.commandBufferCount = 1;
    si.pCommandBuffers = &cmd;
    r = ((PFN_vkQueueSubmit)load_fn("vkQueueSubmit"))(queue, 1, &si, fence);
    if (r != VK_SUCCESS) fail("vkQueueSubmit", r);
    r = ((PFN_vkWaitForFences)load_fn("vkWaitForFences"))(device, 1, &fence, VK_TRUE, UINT64_MAX);
    if (r != VK_SUCCESS) fail("vkWaitForFences", r);

    for (int i = 0; i < buffer_count; i++) {
        printf("buffer %d:", buffers[i].binding);
        for (uint32_t k = 0; k < buffers[i].count; k++) {
            printf(" ");
            print_value(&buffers[i], k);
        }
        printf("\n");
    }
    ((PFN_vkDestroyDevice)load_fn("vkDestroyDevice"))(device, NULL);
    return 0;
}
