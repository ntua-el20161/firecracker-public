CONFIG=config_vm
SOCKET=/run/firecracker.socket
RUNS=10
total=0
total_migr=0
migr=0
count=0
total_zeroing=0
total_comm=0
total_fake=0
total_offline=0

MEM1=800M
MEM2=256M

GUEST_IP=192.168.100.10

SSH_OPTS="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null"

for i in $(seq 1 $RUNS); do
	echo "Run $i..."
	./launch.sh & 
	sleep 10
	
	ssh $SSH_OPTS root@192.168.100.10 memhog -r1000 $MEM1 & echo $! > /tmp/memhog1.pid
	ssh $SSH_OPTS root@192.168.100.10 memhog -r10000 $MEM2 & echo $! > /tmp/memhog2.pid
	sleep 3

	kill $(cat /tmp/memhog2.pid) && rm -f /tmp/memhog2.pid
    	echo "memhog2 exited, unplugging $MEM2 KiB memory..."

	sudo curl --unix-socket $SOCKET -i -X PATCH 'http://localhost/memory-device' -H 'Accept: application/json' -H 'Content-Type: application/json' -d "{ \"requested_size_kib\": 0}"
	sleep 2
	dur=$(grep -m1 -oP 'total took \K[0-9]+(?= ns)' fc.log)
	migration_dur=$(grep -m1 -oP 'migrate took \K[0-9]+(?= ns)' fc.log)
	migrations=$(grep -m1 -oP 'migrate took \d+ ns for \K\d+(?= pages)' fc.log)
	zeroing=$(grep -m1 -oP 'zeroing took \K[0-9]+(?= ns)' fc.log)
	comm=$(grep -m1 -oP 'comm took \K[0-9]+(?= ns)' fc.log)
	fake=$(grep -m1 -oP 'fake took \K[0-9]+(?= ns)' fc.log)
	offline=$(grep -m1 -oP 'offline took \K[0-9]+(?= ns)' fc.log)
	if [ ! -z "$dur" ]; then
		echo " duration=${dur}ns"
		echo " migrations=${migrations}"
       		total=$(echo "$total + $dur" | bc)
		total_migr=$(echo "$total_migr + $migration_dur" | bc)
		migr=$(echo "$migr + $migrations" | bc)
		total_zeroing=$(echo "$total_zeroing + $zeroing" | bc)
		total_comm=$(echo "$total_comm + $comm" | bc)
		total_fake=$(echo "$total_fake + $fake" | bc)
		total_offline=$(echo "$total_offline + $offline" | bc)
 	        count=$((count+1))
	fi
	sudo pkill firecracker
	rm -f /tmp/memhog1.pid
	sleep 1
	rm fc.log
	sleep 1
done

if [ $count -gt 0 ]; then
    mean=$(echo "scale=3; $total / $count" | bc)
    mean_migr_dur=$(echo "scale=3; $total_migr / $count" | bc)
    mean_migr=$(echo "scale=3; $migr / $count" | bc)
    mean_zeroing=$(echo "scale=3; $total_zeroing / $count" | bc)
    mean_comm=$(echo "scale=3; $total_comm / $count" | bc)
    mean_fake=$(echo "scale=3; $total_fake / $count" | bc)
    mean_offline=$(echo "scale=3; $total_offline / $count" | bc)
    echo "reclaim latency: ${mean}ns"
    echo "reclaim latency: ${mean}ns" | cat >> /home/bill/virtio-mem-sq.txt
    echo "migrations latency: ${mean_migr_dur}ns" | cat >> /home/bill/virtio-mem-sq.txt
    echo "migrations: ${mean_migr}" | cat >> /home/bill/virtio-mem-sq.txt
    echo "zeroing latency: ${mean_zeroing}" | cat >> /home/bill/virtio-mem-sq.txt
    echo "comm latency: ${mean_comm}" | cat >> /home/bill/virtio-mem-sq.txt 
    echo "fake latency: ${mean_fake}" | cat >> /home/bill/virtio-mem-sq.txt
    echo "offline latency: ${mean_offline}" | cat >> /home/bill/virtio-mem-sq.txt
else
    echo "No valid measurements found."
fi

