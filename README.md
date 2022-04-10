# Network Monitor
A utility to monitor network performance

## Build
* [Install `rustup`](https://www.rust-lang.org/tools/install): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
* This project uses nightly features: `rustup install nightly`
* Ensure you're up-to-date (`rustup update`)
* Build the client:\
  `cargo fmt --manifest-path=client/Cargo.toml && cargo +nightly build --manifest-path=client/Cargo.toml`

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
* SSH into the client device
* Create an ssh key on the target device
  * `ssh-keygen -t rsa -b 4096 -C "pi@pi4.local" -f ~/.ssh/id_rsa`
* `cat ~/.ssh/id_rsa.pub` and add it as a readonly deploy key for the repo
* Configure the application by editing `client/config.rs`
* Build the client
  * `cargo +nightly build --release --manifest-path=client/Cargo.toml`
* Copy the client binary to the appropriate folder on the client device
  * `sudo mkdir -p /usr/bin/network-monitor/client/`
  * `cp client/target/release/network-monitor /usr/bin/network-monitor/client/`
* Create a new non-root user to run the service
  * `sudo useradd --system network-monitor`
    * Create a `system`  user, we have no need for interactive shell sessions or a home dir
* Create a service to auto-start the client
  * `sudo cp client/systemd/network-monitor.service /etc/systemd/system/network-monitor.service`
  * `sudo systemctl enable network-monitor.service && sudo systemctl start network-monitor.service`
  * Monitor service health:
    * `sudo systemctl status network-monitor.service`
    * `sudo journalctl -u network-monitor | less +G`
  * To restart after binary changes:
    * `sudo systemctl restart network-monitor.service`
* View the network ping logs in a browser at http://localhost:8180 or http://pi4.local:8180
