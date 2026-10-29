/* Native lifecycle control exercised through real Bionic fork/exec and wait. */
#define _GNU_SOURCE
#include <errno.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>
#define NATIVE(n) (0x53430000L+(n))
struct scope_status {uint32_t closing,live,members,reserved;};
static int escape_denied(void) {
    long other=syscall(NATIVE(1320));if(other<0)return -1;
    int denied=syscall(NATIVE(1321),other)==-1;
    if(syscall(NATIVE(102),other))return -1;
    return denied?0:-1;
}
int process_scope_exec_case(const char *reaper_text) {
    pid_t reaper=(pid_t)atoi(reaper_text);
    printf("SCARLET_SCOPE_EXEC pid=%d parent=%d expected=%d\n",getpid(),getppid(),reaper);
    if(reaper<=0 || escape_denied())return 81;
    for(int i=0;i<200;i++) {
        if(getppid()==reaper) {puts("SCARLET_SCOPE_ORPHAN_EXEC_OK");return 17;}
        usleep(10000);
    }
    return 82;
}
static int reaper_case(const char *program,long scope,int output) {
    if(syscall(NATIVE(1321),scope)){printf("SCARLET_SCOPE_FAIL enter errno=%d\n",errno);return 71;}
    if(syscall(NATIVE(1322),scope,0)){printf("SCARLET_SCOPE_FAIL set-reaper errno=%d\n",errno);return 71;}
    if(escape_denied()){printf("SCARLET_SCOPE_FAIL escape-check errno=%d\n",errno);return 71;}
    if(syscall(NATIVE(102),scope))return 72;
    pid_t reaper=getpid(),worker=fork();if(worker<0)return 73;
    if(worker==0) {
        pid_t orphan=fork();if(orphan<0)_exit(74);
        if(orphan==0) {
            char pid[32];snprintf(pid,sizeof(pid),"%d",reaper);
            execl(program,program,"scope-orphan-exec",pid,NULL);perror("scope orphan exec");_exit(75);
        }
        _exit(0);
    }
    int worker_seen=0,orphan_seen=0;
    for(int i=0;i<2;i++) {
        int status=0;pid_t pid=waitpid(-1,&status,0);
        printf("SCARLET_SCOPE_WAIT round=%d pid=%d worker=%d status=%d errno=%d\n",i,pid,worker,status,errno);
        if(pid<0 || !WIFEXITED(status))return 76;
        if(pid==worker && WEXITSTATUS(status)==0)worker_seen++;
        else if(pid!=worker && WEXITSTATUS(status)==17)orphan_seen++;
        else return 77;
    }
    if(worker_seen!=1 || orphan_seen!=1)return 78;
    pid_t pending=fork();if(pending<0)return 79;
    if(pending==0){for(;;)pause();}
    uint32_t ready[2]={(uint32_t)reaper,(uint32_t)pending};
    if(write(output,ready,sizeof(ready))!=sizeof(ready))return 80;
    for(;;)pause();
}
int process_scope_case(const char *program) {
    long scope=syscall(NATIVE(1320));if(scope<0){printf("SCARLET_SCOPE_FAIL create errno=%d\n",errno);return 1;}
    printf("SCARLET_SCOPE_BEGIN handle=%ld\n",scope);
    int ready[2];if(pipe(ready))return 1;
    pid_t reaper=fork();if(reaper<0)return 1;
    if(reaper==0){close(ready[0]);int rc=reaper_case(program,scope,ready[1]);printf("SCARLET_SCOPE_FAIL reaper rc=%d errno=%d\n",rc,errno);_exit(rc);}
    close(ready[1]);uint32_t proof[2]={0};ssize_t count=read(ready[0],proof,sizeof(proof));close(ready[0]);
    int failed=count!=sizeof(proof) || proof[0]!=(uint32_t)reaper || proof[1]==0;
    struct scope_status state={0};
    if(syscall(NATIVE(1324),scope,&state) || state.closing || state.live!=2 || state.members<2 || state.reserved)failed=1;
    if(failed)printf("SCARLET_SCOPE_FAIL proof count=%zd pid=%u child=%u closing=%u live=%u members=%u errno=%d\n",count,proof[0],proof[1],state.closing,state.live,state.members,errno);
    // The outside controller explicitly takes the reaper role before stopping the scope.
    if(syscall(NATIVE(1322),scope,0))failed=1;
    if(syscall(NATIVE(1323),scope))failed=1;
    int status;if(waitpid(reaper,&status,0)!=reaper || !WIFSIGNALED(status) || WTERMSIG(status)!=SIGKILL)failed=1;
    if(!proof[1] || waitpid((pid_t)proof[1],&status,0)!=(pid_t)proof[1] || !WIFSIGNALED(status) || WTERMSIG(status)!=SIGKILL)failed=1;
    int empty=0;
    for(int i=0;i<200;i++) {
        if(syscall(NATIVE(1324),scope,&state)){failed=1;break;}
        if(state.closing==1 && state.live==0 && state.reserved==0){empty=1;break;}
        usleep(10000);
    }
    if(syscall(NATIVE(102),scope))failed=1;
    if(failed || !empty)return 1;
    puts("SCARLET_SCOPE_OK orphan-reaper fork exec retained-without-handle no-escape terminate outside-survives");
    return 0;
}
