cargo bootimage --target x86_64-sos.json && echo "=== FINISHED COMPILING, RUNNING WITH QEMU ===" && \
	qemu-system-x86_64 \
	-drive file=target/x86_64-sos/debug/bootimage-sos.bin,format=raw,if=ide,index=0 \
	-drive file=disk.img,format=raw,if=ide,index=1 \
	-boot order=c \
	-serial stdio \
	-device virtio-gpu-pci \
	-display sdl\
	-vga none
