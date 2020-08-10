#include <sys/types.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <unistd.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>

int main()
{
    pid_t pid;
    int cnt = 0;
    int pipefd[2];
    char buf;
    char w[12];
    char r[12];
    if (pipe(pipefd) == -1) {
        printf("pipe");
        exit(-1);
    }
    sprintf(w,"%d",pipefd[1]);
    sprintf(r,"%d",pipefd[0]);
    pid = vfork();
    if(pid<0)
        printf("error in fork!\n");
    else if(pid == 0)
    {	
    	execl("/bin/testpipe2","/bin/testpipe2",r,w,NULL);
    	exit(0);
    }
    else if(pid > 0)
    {	
       close(pipefd[1]);
       int fd = open("testpipe.txt", O_WRONLY | O_CREAT);
       while (read(pipefd[0], &buf, 1) > 0)
          write(fd, &buf, 1);
       write(fd, "\n", 1);
       close(pipefd[0]);  
       close(fd);
    }
    return 0;
}
