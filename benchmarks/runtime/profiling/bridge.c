/* Diagnostic-only gperftools control. A synchronous socket handshake prevents
 * profiling cuts from racing benchmark phases. Never loaded by production. */
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#include <pthread.h>
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
static int server;
static const char *directory;
static void *control(void *unused) {
  (void)unused;
  void (*stop)(void) = dlsym(RTLD_DEFAULT, "ProfilerStop");
  int (*start)(const char *) = dlsym(RTLD_DEFAULT, "ProfilerStart");
  void (*dump)(const char *) = dlsym(RTLD_DEFAULT, "HeapProfilerDump");
  if (!stop && !dump) abort();
  for (;;) {
    int fd = accept(server, NULL, NULL);
    if (fd < 0) abort();
    char phase[128]; size_t n = 0;
    while (n + 1 < sizeof phase) {
      if (read(fd, phase+n, 1) != 1) abort();
      if (phase[n++] == '\n') break;
    }
    if (!n || phase[n-1] != '\n') abort();
    phase[n-1] = 0;
    if (strspn(phase,"abcdefghijklmnopqrstuvwxyz_") != n-1) abort();
    if (stop) stop();
    if (dump) dump(phase);
    if (start && strcmp(phase,"after_cleanup")) {
      char path[4096];
      int len = snprintf(path,sizeof path,"%s/%s.cpu",directory,phase);
      if (len < 0 || len >= (int)sizeof path || !start(path)) abort();
    }
    if (write(fd,"K",1) != 1) abort();
    close(fd);
  }
  return NULL;
}
__attribute__((constructor)) static void initialize(void) {
  const char *path = getenv("RUSTX_BENCH_PROFILE_SOCKET");
  if (!path) return;
  directory = getenv("RUSTX_BENCH_PROFILE_DIR");
  if (!directory) abort();
  struct sockaddr_un address = {.sun_family=AF_UNIX};
  if (strlen(path) >= sizeof address.sun_path) abort();
  strcpy(address.sun_path,path);
  server = socket(AF_UNIX,SOCK_STREAM,0);
  if (server < 0 || bind(server,(struct sockaddr*)&address,sizeof address) || listen(server,1)) abort();
  pthread_t thread;
  if (pthread_create(&thread,NULL,control,NULL) || pthread_detach(thread)) abort();
}
