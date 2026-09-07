AS = riscv64-linux-gnu-as
LD = riscv64-linux-gnu-ld
OBJCOPY = riscv64-linux-gnu-objcopy

CFLAGS = -march=rv64gc -mabi=lp64d
LDFLAGS = -T hello.lds

all: hello.bin

hello.o: hello.s
	$(AS) $(CFLAGS) -o $@ $<

hello.elf: hello.o
	$(LD) $(LDFLAGS) -o $@ $<

hello.bin: hello.elf
	$(OBJCOPY) -O binary $< $@

clean:
	rm -f *.o *.elf *.bin

.PHONY: all clean
