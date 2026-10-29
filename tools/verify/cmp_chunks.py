import struct,sys,collections
S=sys.argv[1]
def load(p):
    pal=open(p+'.palette').read().split('\n'); d=open(p,'rb').read()
    n=len(d)//2; v=struct.unpack('>%dH'%n,d); return [pal[i] for i in v]
a=load(S+'/java_chunks.bin'); b=load(S+'/rust_chunks.bin')
assert len(a) == len(b), (len(a), len(b))
total=0
per=98304
for c in range(len(a)//per):
    diffs=[i for i in range(c*per,(c+1)*per) if a[i]!=b[i]]
    kinds=collections.Counter((a[i],b[i]) for i in diffs)
    ex=[]
    for i in diffs[:6]:
        j=i-c*per; col=j//384; y=j%384-64; ex.append((col&15,y,col>>4,a[i],b[i]))
    total+=len(diffs)
    if diffs: print('chunk',c,'diffs',len(diffs),kinds.most_common(5),ex)
print(f'{len(a)//per} chunks compared, {total} blocks differ')
