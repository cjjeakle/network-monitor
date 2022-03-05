# Network Monitor

A utility to monitor network performance

## Build

`rustfmt client.rs && rustc client.rs`

## Deploy

### Server Side
* Copy `server-files/index.html` into `/var/www/html/network-monitor/` on the server
* Configure nginx to serve that file: `/etc/nginx/conf.d/network-monitor.conf`
```
server {
    listen 80;
    server_name ping.projects.chrisjeakle.com;
    root /var/www/html/network-monitor;
    index index.html;
}
```

### Client Side
* Create a service: `/etc/systemd/system/network-monitor.service`
```
[Unit]
Description=Network Monitor Client

[Service]
Type=simple
Restart=always
User=network-monitor
Group=network-monitor
WorkingDirectory=/usr/bin/network-monitor
ExecStart=/usr/bin/network-monitor/client

[Install]
WantedBy=multi-user.target
```
* View the logging via a browser
  * http://localhost:8080