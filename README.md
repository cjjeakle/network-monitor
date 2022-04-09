# Network Monitor
A utility to monitor network performance

## Build
* [Install `rustup`](https://www.rust-lang.org/tools/install)
* Update (`rustup update`)
* Build the client:
  ```
  cd client
  cargo fmt && cargo build
  ```

## Deploy

### Server Side
* Set up the web pages:
  * On the server: `ssh root@projects.chrisjeakle.com 'mkdir /var/www/html/ping/'`
  * `scp -pr /server/html root@projects.chrisjeakle.com:/var/www/html/ping/` on the server
* Configure nginx:
  * `scp server/nginx/ping.conf root@projects.chrisjeakle.com:/etc/nginx/conf.d/ping.conf`
  * On the server: `ssh root@projects.chrisjeakle.com 'service nginx reload'`
* Test the config
  * `ping -c 4 ping.projects.chrisjeakle.com`
  * Ensure there's no redirects: `curl -H 'Cache-Control: no-cache' http://ping.projects.chrisjeakle.com/ping/ -I -k`
  * `curl http://ping.projects.chrisjeakle.com/ping/`
  * Visit in a browser: http://ping.projects.chrisjeakle.com/

### Client Side
* Configure the application by editing `client/config.rs`
* Build the client
* Copy the client binary to `/usr/bin/network-monitor/client` on the client device
* Create a service to auto-start the client
  * Copy `client/systemd/network-monitor.service` to `/etc/systemd/system/network-monitor.service` on the client device
* View the logging via a browser
  * http://localhost:8180
