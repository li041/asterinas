## 启动`asterinas`
```sh
# 启动容器
docker run -id --name lxh -it --privileged --network=host -v /dev:/dev -v $(pwd)/asterinas:/root/asterinas asterinas/asterinas:0.17.0-20251228
# 其中-v, 把~/xinhao/asterinas挂载东容器的/root/asterinas目录
# 容器中的 `/root/asterinas` 目录将与宿主机的 `$(pwd)/asterinas` 目录同步，容器可以访问和修改该目录中的内容

# 进入容器
sudo docker exec -it lxh /bin/bash
# /root/asterinas下
make kernel 
make run_kernel
```
- 可以直接进入远程容器, `remote-ssh` + `dev container`
	- 先ssh远程连接上server, 然后`attach to running container`
## 测试
- `make run_kernel`使用的`initramfs.cpio.gz`作为rootfs
- `make run_kernel AUTO_TEST=syscall`, 会根据``

```
https://github.com/li041/asterinas.git
  git config --global user.email "you@example.com"

  git config --global user.name "Your Name"
```

- 提pr前先本地`make check`等
	- 具体可以看`.github`下, 这个是github ci相关配置, 先跑一下
- format方法
	- `clang-format -i ./test/initramfs/src/apps/process/pidfd_getfd.c`
- 