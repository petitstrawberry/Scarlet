#include <property_info_serializer/property_info_serializer.h>
#include <sys/system_properties.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <unistd.h>
#include <cstdio>
#include <fstream>
#include <iterator>
#include <map>
#include <string>
#include <vector>

extern "C" int __system_property_area_init();
extern "C" int __system_property_add(const char*, unsigned, const char*, unsigned);

static std::string read_file(const std::string& path) {
    std::ifstream file(path);
    if (!file) { fprintf(stderr, "cannot read %s\n", path.c_str()); exit(1); }
    return {std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>()};
}

int main(int argc, char** argv) {
    if (argc != 3) return 2;
    std::vector<android::properties::PropertyInfoEntry> contexts;
    std::ifstream context_paths(argv[1]);
    if (!context_paths) return 3;
    std::string path;
    while (std::getline(context_paths, path)) {
        std::vector<std::string> errors;
        android::properties::ParsePropertyInfoFile(read_file(path), true, &contexts, &errors);
        for (const auto& error : errors) fprintf(stderr, "%s: %s\n", path.c_str(), error.c_str());
        if (!errors.empty()) return 4;
    }
    std::string serialized, error;
    if (!android::properties::BuildTrie(contexts, "u:object_r:default_prop:s0", "string", &serialized, &error)) {
        fprintf(stderr, "BuildTrie: %s\n", error.c_str()); return 5;
    }
    int fd = open("/dev/__properties__/property_info", O_WRONLY | O_CREAT | O_EXCL, 0444);
    if (fd < 0) { perror("property_info"); return 6; }
    size_t offset = 0;
    while (offset < serialized.size()) {
        ssize_t n = write(fd, serialized.data() + offset, serialized.size() - offset);
        if (n <= 0) return 7;
        offset += n;
    }
    if (close(fd)) return 8;
    int init_status = __system_property_area_init();
    if (init_status) {
        fprintf(stderr, "Bionic property area initialization failed\n"); return 9;
    }
    std::map<std::string, std::string> properties;
    std::ifstream values(argv[2]);
    if (!values) return 10;
    std::string line;
    while (std::getline(values, line)) {
        if (line.empty() || line[0] == '#') continue;
        auto at = line.find('=');
        if (at == std::string::npos || at == 0) return 11;
        properties[line.substr(0, at)] = line.substr(at + 1);
    }
    for (const auto& [name, value] : properties) {
        if (__system_property_add(name.data(), name.size(), value.data(), value.size())) {
            fprintf(stderr, "cannot add %s\n", name.c_str()); return 12;
        }
        const prop_info* pi = __system_property_find(name.c_str());
        if (!pi) return 13;
        std::string observed;
        __system_property_read_callback(pi, [](void* data, const char*, const char* v, uint32_t) {
            *static_cast<std::string*>(data) = v;
        }, &observed);
        if (observed != value) { fprintf(stderr, "property mismatch: %s\n", name.c_str()); return 14; }
    }
    printf("SCARLET_PROPERTY_SNAPSHOT_OK %zu\n", properties.size());
    return 0;
}
