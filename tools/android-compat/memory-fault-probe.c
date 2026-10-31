/* Real AArch64 faults, Linux siginfo/ucontext and resumable handler state. */
#define _GNU_SOURCE
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <ucontext.h>
#include <unistd.h>
#define CHECK(x) do {if(!(x)){perror(#x);return 1;}} while(0)
#define MAGIC UINT64_C(0x76325410)
__asm__(".text\n.p2align 2\n"
        ".global fault_load\nfault_load:\nfmov d0,x1\n"
        ".global fault_load_pc\nfault_load_pc:\nldr x0,[x0]\nfmov x1,d0\nstr x1,[x2]\nret\n"
        ".global fault_store\nfault_store:\n"
        ".global fault_store_pc\nfault_store_pc:\nstr x1,[x0]\nret\n"
        ".global fault_resume\nfault_resume:\nret\n");
extern uint64_t fault_load(void *,uint64_t,uint64_t *);
extern uint64_t fault_store(void *,uint64_t);
extern char fault_load_pc[],fault_store_pc[],fault_resume[];
static uintptr_t alt_begin,alt_end,expected_address,expected_pc,resume_pc;
static int expected_code,expected_execute,expected_write;
static volatile sig_atomic_t caught;
static void handler(int signal,siginfo_t *info,void *context) {
    ucontext_t *uc=context;uintptr_t local=(uintptr_t)&uc;
    if(signal!=SIGSEGV || info->si_signo!=SIGSEGV || info->si_errno || info->si_code!=expected_code ||
       (uintptr_t)info->si_addr!=expected_address || uc->uc_mcontext.fault_address!=expected_address ||
       uc->uc_mcontext.pc!=expected_pc || local<alt_begin || local>=alt_end) _exit(81);
    const unsigned char *records=uc->uc_mcontext.__reserved;int esr_seen=0,fpsimd_seen=0;
    for(size_t at=0;at+8<=sizeof(uc->uc_mcontext.__reserved);) {
        uint32_t magic,size;memcpy(&magic,records+at,4);memcpy(&size,records+at+4,4);
        if(!magic && !size)break;
        if(size<16 || size%16 || size>sizeof(uc->uc_mcontext.__reserved)-at)_exit(82);
        if(magic==0x46508001)fpsimd_seen++;
        if(magic==0x45535201) {
            uint64_t esr;memcpy(&esr,records+at+8,8);
            if(size!=16 || (esr>>26)!=(expected_execute?0x20u:0x24u) ||
               (!expected_execute && (int)((esr>>6)&1)!=expected_write))_exit(83);
            esr_seen++;
        }
        at+=size;
    }
    if(esr_seen!=1 || fpsimd_seen!=1)_exit(84);
    caught++;
    uc->uc_mcontext.regs[0]=MAGIC;uc->uc_mcontext.pc=resume_pc;
    /* The saved FPSIMD state must restore d0 after this deliberate clobber. */
    __asm__ volatile("movi v0.16b,#0x33":::"v0");
}
static void expect(void *address,uintptr_t pc,int code,int execute,int write) {
    expected_address=(uintptr_t)address;expected_pc=pc;expected_code=code;
    expected_execute=execute;expected_write=write;resume_pc=pc+4;
}
int main(void) {
    setvbuf(stdout,NULL,_IONBF,0);
    size_t page=(size_t)sysconf(_SC_PAGESIZE);
    void *alt=mmap(NULL,65536,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);CHECK(alt!=MAP_FAILED);
    alt_begin=(uintptr_t)alt;alt_end=alt_begin+65536;
    stack_t stack={.ss_sp=alt,.ss_size=65536};CHECK(sigaltstack(&stack,NULL)==0);
    struct sigaction action={.sa_sigaction=handler,.sa_flags=SA_SIGINFO|SA_ONSTACK};
    CHECK(sigemptyset(&action.sa_mask)==0 && sigaction(SIGSEGV,&action,NULL)==0);
    uint64_t sentinel=UINT64_C(0x71935abce02468df),observed=0;
    expect(NULL,(uintptr_t)fault_load_pc,SEGV_MAPERR,0,0);
    CHECK(fault_load(NULL,sentinel,&observed)==MAGIC && observed==sentinel && caught==1);
    void *region=mmap(NULL,page,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);CHECK(region!=MAP_FAILED);
    *(uint64_t *)region=123;CHECK(mprotect(region,page,PROT_NONE)==0);
    expect(region,(uintptr_t)fault_load_pc,SEGV_ACCERR,0,0);
    CHECK(fault_load(region,sentinel,&observed)==MAGIC && observed==sentinel && caught==2);
    CHECK(mprotect(region,page,PROT_READ)==0);
    expect(region,(uintptr_t)fault_store_pc,SEGV_ACCERR,0,1);
    CHECK(fault_store(region,456)==MAGIC && *(uint64_t *)region==123 && caught==3);
    CHECK(mprotect(region,page,PROT_READ|PROT_WRITE)==0);
    *(uint32_t *)region=0xd65f03c0; /* RET, on an NX page. */
    expect(region,(uintptr_t)region,SEGV_ACCERR,1,0);resume_pc=(uintptr_t)fault_resume;
    CHECK(((uint64_t (*)(void))region)()==MAGIC && caught==4);
    sigset_t mask;CHECK(sigprocmask(SIG_SETMASK,NULL,&mask)==0 && sigismember(&mask,SIGSEGV)==0);
    CHECK(munmap(region,page)==0);
    stack.ss_flags=SS_DISABLE;CHECK(sigaltstack(&stack,NULL)==0 && munmap(alt,65536)==0);
    puts("SCARLET_MEMORY_FAULT_OK maperr protection-read protection-write nx altstack fpsimd sigreturn");
    return 0;
}
