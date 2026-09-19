#define _GNU_SOURCE
#include <stdlib.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include <fcntl.h>
#include <sys/mman.h>
#include <unistd.h>

void hexdump(const void *data, size_t size)
{
    const uint8_t *bytes = (const uint8_t *)data;
    for (size_t i = 0; i < size; i++) {
        if (i % 32 == 0) {
            printf("%08zx  ", i);
        }
        printf("%02x ", bytes[i]);
        if (i % 32 == 31 || i == size - 1) {
            printf("\n");
        }
    }
}

#define SIZE 4096

int main()
{
    __off_t offset = 0;


    size_t pgz = getpagesize();
    printf("Page size: %zu\n", pgz);
    if (pgz != SIZE) {
        printf("Page size is not %d\n", SIZE);
        exit(1);
    }

    void *addr = mmap(
        NULL, 2 * SIZE, PROT_NONE, MAP_ANONYMOUS | MAP_PRIVATE, -1, offset);
    if (addr == MAP_FAILED) {
        perror("mmap");
        return 1;
    }

    int fd = memfd_create("log", MFD_ALLOW_SEALING);
    perror("memfd_create");
    ftruncate(fd, SIZE); /* set its size first */

    void *addr1 = mmap(
        addr, SIZE, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_FIXED, fd, offset);
    if (addr1 == MAP_FAILED) {
        perror("mmap1");
        return 1;
    }

    void *addr2 = mmap(
        addr + SIZE, SIZE, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_FIXED, fd, offset);
    if (addr2 == MAP_FAILED) {
        perror("mmap2");
        return 1;
    }

    printf("addr: %p addr1: %p addr2: %p\n", addr, addr1, addr2);

    strcpy(addr1, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA!");
    strcpy(addr2, "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB!");
    hexdump(addr, 2 * SIZE);

    printf("done\n");

    return 0;
}