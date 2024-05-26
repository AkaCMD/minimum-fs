echo "Hello, World..............................................................................." | sudo tee /mnt/hello.txt
stat /mnt/hello.txt
echo "Hello, Filesystem..............................................................................." | sudo tee /mnt/file.txt
stat /mnt/file.txt

sudo mkdir /mnt/my_folder
echo "I'm file #1..............................................................................." | sudo tee /mnt/my_folder/file_1.txt
stat /mnt/my_folder/file_1.txt
echo "I'm file #2..............................................................................." | sudo tee /mnt/my_folder/file_2.txt
stat /mnt/my_folder/file_2.txt
echo "I'm file #3..............................................................................." | sudo tee /mnt/my_folder/file_3.txt
stat /mnt/my_folder/file_3.txt

sudo sync /mnt
