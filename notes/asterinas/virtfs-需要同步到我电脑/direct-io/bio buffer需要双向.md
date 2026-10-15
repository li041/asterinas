- 对于非sector对齐写入, 目前采取`read-modify-write`策略, 这样对于`DmaBuf direction`的要求就是`FromAndToDevice`![[Pasted image 20260807141814.png]]
- 这样block会构造
```
segment 0: FromAndToDevice 先读sector 0, 修改然后写回
segment 1..end: ToDevice, 对齐
segment end: 先读sector end, 修改然写回
```
- 所以引入了`DmaStreamSliceRef`
![[Pasted image 20260807142157.png]]
## Block层
- `DeviceInner`层`init`时预分配`block_requests`和`block_response`控制缓冲区