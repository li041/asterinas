- 
```
# 安装必要工具
sudo apt-get install -y apt-transport-https ca-certificates curl gnupg
# 添加 Kubernetes GPG 密钥
curl -fsSL https://pkgs.k8s.io/core:/stable:/v1.30/deb/Release.key | sudo gpg --dearmor -o /etc/apt/keyrings/kubernetes-apt-keyring.gpg
# 添加 Kubernetes APT 仓库
echo 'deb [signed-by=/etc/apt/keyrings/kubernetes-apt-keyring.gpg] https://pkgs.k8s.io/core:/stable:/v1.30/deb/ /' | sudo tee /etc/apt/sources.list.d/kubernetes.list
# 禁用swap
sudo swapoff -a
# 安装 Kubernetes 组件
sudo apt-get update
sudo apt-get install -y kubelet kubeadm kubectl
sudo apt-mark hold kubelet kubeadm kubectl

# 初始化单节点集群
# 这个参数定义了k8s集群中Pod网络的IP地址范围，10.244.0.1到10.244.255.254
# 每个Pod在集群中都会被分配这个范围内的一个唯一IP地址
sudo kubeadm init --pod-network-cidr=10.244.0.0/16

# 配置kubectl
mkdir -p $HOME/.kube
sudo cp -i /etc/kubernetes/admin.conf $HOME/.kube/config
sudo chown $(id -u):$(id -g) $HOME/.kube/config

# 移除控制平面节点的污点（允许在此节点上运行工作负载）
kubectl taint nodes --all node-role.kubernetes.io/control-plane-

# 安装 Flannel 网络插件
kubectl apply -f https://github.com/flannel-io/flannel/releases/latest/download/kube-flannel.yml 

```
- docker启动是用的`bash`, 环境不满足