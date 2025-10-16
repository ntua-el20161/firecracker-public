CONFIG=config_vm
SOCKET=/run/firecracker.socket
RUNS=10
total=0
count=0
requested_size=0

for i in $(seq 1 $RUNS); do
	echo "Run $i..."
	./launch.sh & sleep 2
	sudo curl --unix-socket $SOCKET -i -X PATCH 'http://localhost/memory-device' -H 'Accept: application/json' -H 'Content-Type: application/json' -d "{ \"requested_size_kib\": $requested_size }"
	sleep 1
	dur=$(grep -m1 -oP 'total took \K[0-9]+(?= ns)' fc.log)
	if [ ! -z "$dur" ]; then
		echo " duration=${dur}ns"
       		total=$(echo "$total + $dur" | bc)
 	        count=$((count+1))
	fi
	sudo pkill firecracker
	sleep 1
done

if [ $count -gt 0 ]; then
    mean=$(echo "scale=3; $total / $count" | bc)
    echo "Mean duration: ${mean}ns"
    echo "Mean duration virtio-mem: ${mean}" | cat >> /home/bill/perf.txt 
else
    echo "No valid measurements found."
fi

