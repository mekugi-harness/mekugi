#ifdef __APPLE__
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <spawn.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#define ENV_SOCK "MEKUGI_WRITE_OBSERVER_SOCK"

static bool is_write_flags(int flags) {
    return (flags & (O_WRONLY | O_RDWR | O_CREAT | O_TRUNC)) != 0;
}

static bool is_sip_binary(const char *path) {
    if (!path) return false;
    return strncmp(path, "/bin/", 5) == 0 ||
           strncmp(path, "/usr/bin/", 9) == 0 ||
           strncmp(path, "/sbin/", 6) == 0 ||
           strncmp(path, "/usr/sbin/", 10) == 0;
}

static void notify_host(char kind, const char *path) {
    if (!path || !path[0]) return;
    const char *sock_path = getenv(ENV_SOCK);
    if (!sock_path || !sock_path[0]) return;

    int saved_errno = errno;
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) {
        errno = saved_errno;
        return;
    }
    struct sockaddr_un addr;
    memset(&addr, 0, sizeof(addr));
    addr.sun_family = AF_UNIX;
    if (strlen(sock_path) >= sizeof(addr.sun_path)) {
        close(fd);
        errno = saved_errno;
        return;
    }
    strncpy(addr.sun_path, sock_path, sizeof(addr.sun_path) - 1);
    if (connect(fd, (struct sockaddr *)&addr, sizeof(addr)) == 0) {
        size_t len = strlen(path);
        uint8_t header[1] = { (uint8_t)kind };
        uint8_t nul = 0;
        (void)write(fd, header, 1);
        (void)write(fd, path, len);
        (void)write(fd, &nul, 1);
        uint8_t ack = 0;
        (void)read(fd, &ack, 1);
    }
    close(fd);
    errno = saved_errno;
}

static void notify_path_at(int dirfd, const char *path) {
    if (!path || !path[0]) return;
    if (path[0] == '/') {
        notify_host('W', path);
        return;
    }
    char base[PATH_MAX];
    bool ok = false;
    if (dirfd == AT_FDCWD) {
        ok = (getcwd(base, sizeof(base)) != NULL);
    } else {
        ok = (fcntl(dirfd, F_GETPATH, base) == 0);
    }
    if (!ok) return;
    char full[PATH_MAX];
    size_t blen = strlen(base);
    size_t plen = strlen(path);
    if (blen + 1 + plen + 1 >= sizeof(full)) return;
    memcpy(full, base, blen);
    full[blen] = '/';
    memcpy(full + blen + 1, path, plen + 1);
    notify_host('W', full);
}

static int my_open(const char *path, int oflag, ...) {
    mode_t mode = 0;
    if (oflag & O_CREAT) {
        va_list ap;
        va_start(ap, oflag);
        mode = (mode_t)va_arg(ap, int);
        va_end(ap);
    }
    if (is_write_flags(oflag)) {
        notify_path_at(AT_FDCWD, path);
    }
    return (oflag & O_CREAT) ? open(path, oflag, mode) : open(path, oflag);
}

static int my_openat(int dirfd, const char *path, int oflag, ...) {
    mode_t mode = 0;
    if (oflag & O_CREAT) {
        va_list ap;
        va_start(ap, oflag);
        mode = (mode_t)va_arg(ap, int);
        va_end(ap);
    }
    if (is_write_flags(oflag)) {
        notify_path_at(dirfd, path);
    }
    return (oflag & O_CREAT) ? openat(dirfd, path, oflag, mode) : openat(dirfd, path, oflag);
}

static int my_unlink(const char *path) {
    notify_path_at(AT_FDCWD, path);
    return unlink(path);
}

static int my_unlinkat(int dirfd, const char *path, int flag) {
    notify_path_at(dirfd, path);
    return unlinkat(dirfd, path, flag);
}

static int my_rename(const char *oldpath, const char *newpath) {
    notify_path_at(AT_FDCWD, oldpath);
    notify_path_at(AT_FDCWD, newpath);
    return rename(oldpath, newpath);
}

static int my_renameat(int oldfd, const char *oldpath, int newfd, const char *newpath) {
    notify_path_at(oldfd, oldpath);
    notify_path_at(newfd, newpath);
    return renameat(oldfd, oldpath, newfd, newpath);
}

static int my_truncate(const char *path, off_t length) {
    notify_path_at(AT_FDCWD, path);
    return truncate(path, length);
}

static int my_execve(const char *path, char *const argv[], char *const envp[]) {
    if (is_sip_binary(path)) {
        notify_host('S', path);
    }
    return execve(path, argv, envp);
}

static int my_posix_spawn(
    pid_t *pid,
    const char *path,
    const posix_spawn_file_actions_t *file_actions,
    const posix_spawnattr_t *attrp,
    char *const argv[],
    char *const envp[]
) {
    if (is_sip_binary(path)) {
        notify_host('S', path);
    }
    return posix_spawn(pid, path, file_actions, attrp, argv, envp);
}

#define DYLD_INTERPOSE(_replacement, _replacee) \
    __attribute__((used)) static struct { \
        const void *replacement; \
        const void *replacee; \
    } _interpose_##_replacee __attribute__((section("__DATA,__interpose"))) = { \
        (const void *)(unsigned long)&_replacement, \
        (const void *)(unsigned long)&_replacee \
    };

DYLD_INTERPOSE(my_open, open)
DYLD_INTERPOSE(my_openat, openat)
DYLD_INTERPOSE(my_unlink, unlink)
DYLD_INTERPOSE(my_unlinkat, unlinkat)
DYLD_INTERPOSE(my_rename, rename)
DYLD_INTERPOSE(my_renameat, renameat)
DYLD_INTERPOSE(my_truncate, truncate)
DYLD_INTERPOSE(my_execve, execve)
DYLD_INTERPOSE(my_posix_spawn, posix_spawn)
#endif
