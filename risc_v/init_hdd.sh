fallocate -l 32M hdd.dsk
sudo losetup /dev/loop24 hdd.dsk
sudo mkfs.minix -3 /dev/loop24
sudo mount /dev/loop24 /mnt
sudo sync /mnt