#!/usr/bin/env bash

cat > target/doc/index.html << EOF
<html>
    <head>
        <meta http-equiv="refresh" content="0;URL=kernel_hal/index.html">
        <title>Redirection</title>
    </head>
    <body onload="window.location = 'kernel_hal/index.html'">
        <p>Redirecting to <a href="kernel_hal/index.html">kernel_hal/index.html</a>...</p>
    </body>
</html>
EOF