// SPDX-License-Identifier: MPL-2.0

#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/prctl.h>
#include <sys/wait.h>
#include <unistd.h>

static volatile sig_atomic_t sigchld_count;

static void sigchld_handler(int signum)
{
	(void)signum;
	sigchld_count++;
}

static void fail_errno(const char *message)
{
	perror(message);
	exit(EXIT_FAILURE);
}

static void fail_status(const char *message, int status)
{
	fprintf(stderr, "%s: status=%d\n", message, status);
	exit(EXIT_FAILURE);
}

int main(void)
{
	struct sigaction action = {
		.sa_handler = sigchld_handler,
	};
	int status;
	int subreaper = -1;
	int sync_pipe[2];
	char byte = 0;
	pid_t child_pid;
	pid_t waited_pid;
	pid_t subreaper_pid = getpid();

	if (sigemptyset(&action.sa_mask) < 0)
		fail_errno("sigemptyset");
	if (sigaction(SIGCHLD, &action, NULL) < 0)
		fail_errno("sigaction(SIGCHLD)");

	if (prctl(PR_GET_CHILD_SUBREAPER, &subreaper) < 0)
		fail_errno("prctl(PR_GET_CHILD_SUBREAPER)");
	if (subreaper != 0)
		fail_status("unexpected initial child-subreaper state", subreaper);

	if (prctl(PR_SET_CHILD_SUBREAPER, 1) < 0)
		fail_errno("prctl(PR_SET_CHILD_SUBREAPER)");

	if (prctl(PR_GET_CHILD_SUBREAPER, &subreaper) < 0)
		fail_errno("prctl(PR_GET_CHILD_SUBREAPER)");
	if (subreaper != 1)
		fail_status("failed to enable child-subreaper state", subreaper);

	if (pipe(sync_pipe) < 0)
		fail_errno("pipe");

	child_pid = fork();
	if (child_pid < 0)
		fail_errno("fork");

	if (child_pid == 0) {
		pid_t orphan_pid = fork();

		if (orphan_pid < 0)
			fail_errno("fork");

		if (orphan_pid == 0) {
			close(sync_pipe[1]);
			if (read(sync_pipe[0], &byte, sizeof(byte)) != sizeof(byte))
				fail_errno("read");
			if (getppid() != subreaper_pid) {
				fprintf(stderr,
					"orphan was not reparented: ppid=%d expected=%d\n",
					getppid(), subreaper_pid);
				_exit(EXIT_FAILURE);
			}
			_exit(EXIT_SUCCESS);
		}

		close(sync_pipe[0]);
		close(sync_pipe[1]);

		if (prctl(PR_GET_CHILD_SUBREAPER, &subreaper) < 0)
			fail_errno("prctl(PR_GET_CHILD_SUBREAPER)");
		if (subreaper != 0)
			fail_status("child inherited child-subreaper state", subreaper);

		_exit(EXIT_SUCCESS);
	}

	close(sync_pipe[0]);

	if (waitpid(child_pid, &status, 0) != child_pid)
		fail_errno("waitpid");
	if (!WIFEXITED(status) || WEXITSTATUS(status) != 0)
		fail_status("child exited abnormally", status);

	if (write(sync_pipe[1], &byte, sizeof(byte)) != sizeof(byte))
		fail_errno("write");
	close(sync_pipe[1]);

	waited_pid = wait(&status);
	if (waited_pid < 0)
		fail_errno("wait");
	if (!WIFEXITED(status) || WEXITSTATUS(status) != 0)
		fail_status("orphan exited abnormally", status);

	if (sigchld_count != 2) {
		fprintf(stderr, "expected 2 SIGCHLD signals, got %d\n",
			sigchld_count);
		return EXIT_FAILURE;
	}

	return EXIT_SUCCESS;
}
