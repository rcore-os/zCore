#ifdef HAVE_GETRANDOM
 #include <sys/random.h>
#else
 #include <syscall.h>
 #include <linux/random.h>
#endif
 #include <stdio.h>

 int main(){
    int buf;
#ifdef HAVE_GETRANDOM
    getrandom((void *)&buf,sizeof(buf),GRND_RANDOM);
#else
    syscall(SYS_getrandom, (void *)&buf, sizeof(buf), GRND_RANDOM);
#endif
    printf("random: %d\n",buf);
    return 0;
 }
