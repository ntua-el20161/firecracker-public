#include <stdio.h>
#include <stdlib.h>

#define SIZE (1024UL*1024*1024) // 1 GiB
#define PAGE_SIZE 4096

int main() {
    printf("Allocating 1 GiB...\n");
    unsigned char *mem = malloc(SIZE);
    if (!mem) {
        perror("malloc");
        return 1;
    }

    printf("Touching memory...\n");
    for (size_t i = 0; i < SIZE; i += PAGE_SIZE) {
        mem[i] = 1;  // write a byte per page
    }

    printf("Done. Press Enter to free memory...\n");
    getchar();
    
    system("free -h");   

    free(mem);
    return 0;
}

