# Network Monitor
A utility to monitor network performance

## Build
`rustfmt client.rs && rustc client.rs`

## Deploy

### Server Side
* Copy `server/index.html` to `/var/www/html/ping/index.html` on the server
* Configure nginx by copying `server/ping.conf` to `/etc/nginx/conf.d/ping.conf`

### Client Side
* Configure the application by editing `client/config.rs`
* Build
* Copy the client binary to `/usr/bin/network-monitor/client` on the client device
* Create a service to auto-start the client
  * Copy `client/network-monitor.service` to `/etc/systemd/system/network-monitor.service` on the client device
* View the logging via a browser
  * http://localhost:8080
