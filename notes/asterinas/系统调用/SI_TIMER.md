- `siginfo`的`__sifields`(`union`), 根据不同类型的定义
- 创建`POSIX.1b interval` timer
	- `kernel/time/posix-timers.c`
- [overrun语义](https://stackoverflow.com/questions/5646081/what-does-timer-getoverrun-do)
	- the number of signals not generated
- [overrun进一步阐述](https://www.faschingbauer.me/trainings/material/soup/linux/sysprog/timers/posix-overrun/index.html)
	- 
- `overrun`是动态的, 当信号真正要发送的时才计算
	- `__posixtimer_deliver_signal`时计算
## Asterinas
- `update_cpu_time`当时钟中断到来的时候调用, 处理过时的定时器触发回调
- 定时器结构`kernel/src/time/core/timer.rs::struct Timer`
- 定时器管理, `kernel/src/process/process/timer_manager.rs::struct PosixtimerManager`
- 常规的三个的回调都是`create_process_timer_callback`
- `posix_timer`
	- 都是通过`sys_timer_create`创建, 回调是`let send_signal = `
## Linux定时系统
- 1 jiffy = 一个系统定时器中断的时间