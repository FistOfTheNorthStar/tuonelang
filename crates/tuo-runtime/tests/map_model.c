/* Model check of the tuo_rt_map_* shim (`tuo_runtime::map`), driven by
   `tests/map_model.rs`, which compiles this file together with the real
   `map_runtime_c_source()`.

   Seeded random insert/get/remove/keys sequences run against an
   insertion-ordered association list -- the reference interpreter's map
   semantics -- over both key kinds and both value strides, asserting every
   observable (found flags, returned values, `len`, `keys` order) and that
   every block the shim allocates is freed exactly once. TUO_SENTINEL is the
   allocator's zero-size sentinel, passed in by the driver. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
static long outstanding = 0;
void *tuo_rt_alloc(size_t size, size_t align) { (void)align; outstanding++; void *p = malloc(size); memset(p, 0xAB, size); return p; }
void tuo_rt_dealloc(void *ptr, size_t size, size_t align) { (void)size; (void)align; outstanding--; free(ptr); }
void tuo_rt_map_int_insert(long long *, long long, const void *, unsigned long long, long long *);
void tuo_rt_map_int_get(const long long *, long long, unsigned long long, long long *);
void tuo_rt_map_int_remove(long long *, long long, unsigned long long, long long *);
void tuo_rt_map_int_keys(const long long *, unsigned long long, long long *);
void tuo_rt_map_str_insert(long long *, const unsigned char *, unsigned long long, const void *, unsigned long long, long long *);
void tuo_rt_map_str_get(const long long *, const unsigned char *, unsigned long long, unsigned long long, long long *);
void tuo_rt_map_str_remove(long long *, const unsigned char *, unsigned long long, unsigned long long, long long *);
void tuo_rt_map_str_keys(const long long *, unsigned long long, long long *);
void tuo_rt_map_drop(long long *, long long);

#define MAXN 4096
static long long mk[MAXN]; static long long mv[MAXN][2]; static int mn;
static int mfind(long long k){ for(int i=0;i<mn;i++) if(mk[i]==k) return i; return -1; }
static char names[256][8]; /* str keys: "k<num>", byte views into this table */
#define FAIL(...) do{ fprintf(stderr, __VA_ARGS__); exit(1);}while(0)

static void run(int is_str, unsigned long long vs, unsigned seed, int ops, int keyspace){
  srand(seed); mn=0;
  long long hdr[3] = {TUO_SENTINEL,0,0};
  long long out[3];
  for(int op=0; op<ops; op++){
    int r = rand()%100; long long k = rand()%keyspace;
    long long val[2] = { rand(), rand() };
    const unsigned char *kp=(const unsigned char*)names[k]; unsigned long long kn=strlen(names[k]);
    int i = mfind(k);
    if (r < 45) {
      if(is_str) tuo_rt_map_str_insert(hdr,kp,kn,val,vs,out); else tuo_rt_map_int_insert(hdr,k,val,vs,out);
      if (i>=0){ if(out[0]!=1 || memcmp(&out[1],mv[i],vs)) FAIL("insert-overwrite mismatch op %d\n",op); memcpy(mv[i],val,vs);} 
      else { if(out[0]!=0) FAIL("insert-new found op %d\n",op); mk[mn]=k; memcpy(mv[mn],val,vs); mn++; }
    } else if (r < 75) {
      if(is_str) tuo_rt_map_str_remove(hdr,kp,kn,vs,out); else tuo_rt_map_int_remove(hdr,k,vs,out);
      if (i>=0){ if(out[0]!=1 || memcmp(&out[1],mv[i],vs)) FAIL("remove mismatch op %d\n",op);
        memmove(&mk[i],&mk[i+1],(mn-i-1)*sizeof mk[0]); memmove(&mv[i],&mv[i+1],(mn-i-1)*sizeof mv[0]); mn--; }
      else if(out[0]!=0) FAIL("remove-absent found op %d\n",op);
    } else if (r < 95) {
      if(is_str) tuo_rt_map_str_get(hdr,kp,kn,vs,out); else tuo_rt_map_int_get(hdr,k,vs,out);
      if (i>=0){ if(out[0]!=1 || memcmp(&out[1],mv[i],vs)) FAIL("get mismatch op %d\n",op);} else if(out[0]!=0) FAIL("get-absent found op %d\n",op);
    } else {
      long long kh[3];
      if(is_str) tuo_rt_map_str_keys(hdr,vs,kh); else tuo_rt_map_int_keys(hdr,vs,kh);
      if(kh[1]!=mn) FAIL("keys len %lld vs %d op %d\n",kh[1],mn,op);
      for(int j=0;j<mn;j++){
        if(is_str){ const unsigned char *p; unsigned long long n; memcpy(&p,(char*)kh[0]+16*j,8); memcpy(&n,(char*)kh[0]+16*j+8,8);
          if(p!=(const unsigned char*)names[mk[j]]||n!=strlen(names[mk[j]])) FAIL("str keys order op %d\n",op);} 
        else if(((long long*)kh[0])[j]!=mk[j]) FAIL("int keys order op %d\n",op);
      }
      if(kh[1]) tuo_rt_dealloc((void*)kh[0],0,8);
    }
    if(hdr[1]!=mn) FAIL("len %lld vs %d op %d\n",hdr[1],mn,op);
  }
  tuo_rt_map_drop(hdr,(is_str?16:8)+vs);
}
/* Removal moves nothing: after removing the first half of a map, each
   survivor still sits at the dense slot it was appended to. (The shim once
   shifted the tail down and rehashed every survivor on each removal --
   quadratic for a drain -- and this is the structural guard against it.) */
static void removal_is_in_place(void){
  long long hdr[3] = {TUO_SENTINEL,0,0}, out[3];
  for(long long k=0;k<1000;k++){ long long v=3*k; tuo_rt_map_int_insert(hdr,k,&v,8,out); }
  const long long *before = (const long long*)hdr[0];
  for(long long k=0;k<500;k++) tuo_rt_map_int_remove(hdr,k,8,out);
  if((const long long*)hdr[0]!=before) FAIL("removal reallocated the entries\n");
  for(long long k=500;k<1000;k++) if(before[2*k]!=k || before[2*k+1]!=3*k) FAIL("removal moved entry %lld\n",k);
  tuo_rt_map_drop(hdr,16);
}

int main(void){
  removal_is_in_place();
  for(int i=0;i<256;i++) snprintf(names[i],8,"k%d",i);
  names[0][0]=0; /* the empty string key too */
  unsigned long long strides[]={8,16};
  for(unsigned seed=1; seed<=400; seed++)
    for(int s=0;s<2;s++) for(int st=0;st<2;st++){
      int ks = (seed%4==0)?4:(seed%4==1)?40:(seed%4==2)?200:256;
      run(s,strides[st],seed,3000,ks);
    }
  if(outstanding!=0) FAIL("leak/double free: %ld outstanding\n",outstanding);
  printf("ok\n"); return 0;
}
