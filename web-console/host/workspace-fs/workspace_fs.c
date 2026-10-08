/* POSIX descriptor operations missing from node:fs. Shared by Linux and macOS. */
#include <node_api.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static napi_value fail(napi_env env, const char *message) {
  napi_throw_error(env, NULL, message);
  return NULL;
}
#define CHECK(call) do { if ((call) != napi_ok) return fail(env, "Invalid workspace filesystem call"); } while (0)

static napi_value open_child(napi_env env, napi_callback_info info) {
  napi_value args[3], result;
  size_t count = 3, length;
  int32_t parent;
  bool directory;
  char name[4097];
  CHECK(napi_get_cb_info(env, info, &count, args, NULL, NULL));
  if (count != 3) return fail(env, "Expected directory descriptor, component and directory flag");
  CHECK(napi_get_value_int32(env, args[0], &parent));
  CHECK(napi_get_value_string_utf8(env, args[1], NULL, 0, &length));
  if (!length || length >= sizeof(name)) return fail(env, "Invalid path component");
  CHECK(napi_get_value_string_utf8(env, args[1], name, sizeof(name), &length));
  if (strlen(name) != length || strchr(name, '/') || !strcmp(name, ".") || !strcmp(name, "..")) return fail(env, "Invalid path component");
  CHECK(napi_get_value_bool(env, args[2], &directory));
  int fd = openat(parent, name, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC | (directory ? O_DIRECTORY : 0));
  if (fd < 0) return fail(env, strerror(errno));
  if (napi_create_int32(env, fd, &result) != napi_ok) { close(fd); return fail(env, "Cannot return descriptor"); }
  return result;
}

static napi_value entries(napi_env env, napi_callback_info info) {
  napi_value arg, result;
  size_t count = 1;
  int32_t fd;
  CHECK(napi_get_cb_info(env, info, &count, &arg, NULL, NULL));
  if (count != 1) return fail(env, "Expected directory descriptor");
  CHECK(napi_get_value_int32(env, arg, &fd));
  CHECK(napi_create_array(env, &result));
  /* Open '.' relative to the held directory for an independent iteration offset. */
  int copy = openat(fd, ".", O_RDONLY | O_DIRECTORY | O_CLOEXEC);
  if (copy < 0) return fail(env, strerror(errno));
  DIR *dir = fdopendir(copy);
  if (!dir) { int error = errno; close(copy); return fail(env, strerror(error)); }
  const char *error = NULL;
  unsigned int index = 0;
  for (;;) {
    errno = 0;
    struct dirent *entry = readdir(dir);
    if (!entry) { if (errno) error = strerror(errno); break; }
    if (!strcmp(entry->d_name, ".") || !strcmp(entry->d_name, "..")) continue;
    if (index == 2000) { error = "Directory exceeds 2000 entries"; break; }
    struct stat st;
    if (fstatat(dirfd(dir), entry->d_name, &st, AT_SYMLINK_NOFOLLOW)) { error = strerror(errno); break; }
    napi_value row, name, directory, link;
    if (napi_create_object(env, &row) != napi_ok ||
        napi_create_buffer_copy(env, strlen(entry->d_name), entry->d_name, NULL, &name) != napi_ok ||
        napi_get_boolean(env, S_ISDIR(st.st_mode), &directory) != napi_ok ||
        napi_get_boolean(env, S_ISLNK(st.st_mode), &link) != napi_ok ||
        napi_set_named_property(env, row, "name", name) != napi_ok ||
        napi_set_named_property(env, row, "directory", directory) != napi_ok ||
        napi_set_named_property(env, row, "link", link) != napi_ok ||
        napi_set_element(env, result, index++, row) != napi_ok) {
      error = "Cannot return directory entry"; break;
    }
  }
  closedir(dir);
  if (error) return fail(env, error);
  return result;
}

static napi_value init(napi_env env, napi_value exports) {
  napi_property_descriptor methods[] = {
    {"openChild", NULL, open_child, NULL, NULL, NULL, napi_default, NULL},
    {"entries", NULL, entries, NULL, NULL, NULL, napi_default, NULL}
  };
  CHECK(napi_define_properties(env, exports, 2, methods));
  return exports;
}
NAPI_MODULE(NODE_GYP_MODULE_NAME, init)
