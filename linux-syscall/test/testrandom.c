 #include <sys/random.h>
 #include <stdio.h>

 int main(){
    int buf;
    getrandom((void *)&buf,sizeof(buf),GRND_RANDOM);
    printf("random: %d\n",buf);
    return 0;
 }