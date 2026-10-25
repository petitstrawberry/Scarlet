"""Bounded loopback TCP relay with fixed propagation delay per direction."""
import argparse,collections,select,socket,threading,time
p=argparse.ArgumentParser();p.add_argument('--listen',type=int,required=True);p.add_argument('--target',type=int,required=True);p.add_argument('--delay-ms',type=float,default=2);a=p.parse_args()
def relay(client):
 peer=socket.create_connection(('127.0.0.1',a.target));sockets=[client,peer]
 for s in sockets:s.setblocking(False);s.setsockopt(socket.IPPROTO_TCP,socket.TCP_NODELAY,1)
 queues=[collections.deque(),collections.deque()];sizes=[0,0];reading=[True,True];write_closed=[False,False]
 try:
  while any(reading) or any(queues):
   now=time.monotonic()
   for i in range(2):
    if not reading[1-i] and not queues[i] and not write_closed[i]:
     sockets[i].shutdown(socket.SHUT_WR);write_closed[i]=True
   reads=[sockets[i] for i in range(2) if reading[i] and sizes[1-i]<1024*1024]
   writes=[sockets[i] for i in range(2) if queues[i] and queues[i][0][0]<=now]
   deadlines=[q[0][0] for q in queues if q and q[0][0]>now]
   timeout=max(0,min(.05,min(deadlines)-now)) if deadlines else .05
   rd,wr,_=select.select(reads,writes,[],timeout)
   for s in rd:
    i=sockets.index(s);data=s.recv(65536)
    if data:queues[1-i].append([time.monotonic()+a.delay_ms/1000,memoryview(data)]);sizes[1-i]+=len(data)
    else:reading[i]=False
   for s in wr:
    i=sockets.index(s);record=queues[i][0]
    try:n=s.send(record[1])
    except BlockingIOError:continue
    sizes[i]-=n;record[1]=record[1][n:]
    if not record[1]:queues[i].popleft()
 except (OSError,ValueError):pass
 finally:
  for s in sockets:s.close()
with socket.socket() as listener:
 listener.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1);listener.bind(('127.0.0.1',a.listen));listener.listen()
 print(f'loopback {a.listen} -> {a.target}, {a.delay_ms} ms delay each direction',flush=True)
 while True:
  client,_=listener.accept();threading.Thread(target=relay,args=(client,),daemon=True).start()
