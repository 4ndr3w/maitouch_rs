Proxy for connecting an ADX touchscreen to a real serial port.

**Why is this needed?**

Despite "talking" at 9600 baud, the ADX microcontroller presents as a USB-CDC serial port.
No real RS-232 communications ever happen, so the baud rate is a meaningless configuration parameter on the port.
It is able to send data at USB speeds and ignore the baud rate.
A simple solution to this problem would be to simply run `socat` or similar to bridge the two ports, but this
breaks down due to one end sending data at a much higher rate than 9600 baud. An additional quirk I've noticed is that the ADX seems to expect to recieve a whole command in a single write() call (which I assume usually maps to one transaction on the bus) and does not seem to perform buffering to handle messages that may be split across multiple writes.

**What does this thing do?**

This means we need to do the following to get this to work:
- Buffer commands from serial to send to the ADX, ensuring that a single command is sent per write() call.
- When the game requests the touchscreen to enter streaming mode, consume the touch updates at USB speeds.
- Send the most up-to-date touch update over serial as fast as we can
