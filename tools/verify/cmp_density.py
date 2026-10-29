import struct,sys
S=sys.argv[1]
a=open(S+'/java_density.bin','rb').read(); b=open(S+'/rust_density.bin','rb').read()
print(len(a),len(b))
n=len(a)//4
ja=struct.unpack('>%di'%n,a); rb=struct.unpack('>%di'%n,b[:len(a)])
fa=struct.unpack('>%df'%n,a); fb=struct.unpack('>%df'%n,b[:len(a)])
vol=16*384*16
for c in range(3):
    diffs=[i for i in range(c*vol,(c+1)*vol) if ja[i]!=rb[i]]
    print('chunk',c,'diffs',len(diffs), [(i-c*vol, fa[i], fb[i]) for i in diffs[:5]])
base=3*vol
names=['final','cont','eros']
for k in range(3):
    diffs=[i for i in range(2000) if ja[base+i*3+k]!=rb[base+i*3+k]]
    print(names[k],'value diffs',len(diffs),[(i,fa[base+i*3+k],fb[base+i*3+k]) for i in diffs[:5]])
