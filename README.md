# Network Monitor
A utility to monitor network performance

## Build
```
cd client
rustfmt client.rs && rustc client.rs
```

## Deploy

### Server Side
* Copy `server/index.html` to `/var/www/html/ping/index.html` on the server
* Configure nginx by copying `server/ping.conf` to `/etc/nginx/conf.d/ping.conf`
* `service nginx reload`
* Test the config
  * `ping -c 4 ping.projects.chrisjeakle.com`
  * Ensure there's no redirects: `curl -H 'Cache-Control: no-cache' http://ping.projects.chrisjeakle.com/ -I -k`
  * `curl http://ping.projects.chrisjeakle.com/`
  * Visit in a browser: http://ping.projects.chrisjeakle.com/

### Client Side
* Configure the application by editing `client/config.rs`
* Build the client
* Copy the client binary to `/usr/bin/network-monitor/client` on the client device
* Create a service to auto-start the client
  * Copy `client/network-monitor.service` to `/etc/systemd/system/network-monitor.service` on the client device
* View the logging via a browser
  * http://localhost:8180
