cargo bootimage && echo "=== FINISHED COMPILING, RUNNING WITH QEMU ===" && qemu-system-x86_64 \
                                    -drive file=target/x86_64-sos/debug/bootimage-sos.bin,format=raw,if=ide,bus=0,unit=0 \
                                    -drive file=disk.img,format=raw,if=ide,bus=0,unit=1 \
                                    -boot order=c \
                                    -serial stdio \
                                    -m 128M
