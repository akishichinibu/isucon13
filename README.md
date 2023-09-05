# isucon13

## bench

webappの土台ができてきたら実装開始

## payment

TBW

## Livestream

HLSで配信予定。動画はS3に配置し、m3u8プレイリストをうまいこと返して複数視聴者が同じタイミングから視聴できるように調停するサーバを用意する予定

## DNS

PowerDNSを用いる予定

## webapp

### DB起動

DBはdocker composeで起動します。

```
isucon/isucon13$ sudo docker compose up -d
[+] Running 3/3
 ⠿ Network isucon13_default    Created                                                                                                                              0.1s
 ⠿ Volume "isucon13_mysql"     Created                                                                                                                              0.0s
 ⠿ Container isucon13-mysql-1  Started                                     
```

起動確認

```
isucon/isucon13$ mysql -h127.0.0.1 -uisucon -pisucon isupipe
mysql: [Warning] Using a password on the command line interface can be insecure.
Reading table information for completion of table and column names
You can turn off this feature to get a quicker startup with -A

Welcome to the MySQL monitor.  Commands end with ; or \g.
Your MySQL connection id is 9
Server version: 8.0.31 MySQL Community Server - GPL

Copyright (c) 2000, 2023, Oracle and/or its affiliates.

Oracle is a registered trademark of Oracle Corporation and/or its
affiliates. Other names may be trademarks of their respective
owners.

Type 'help;' or '\h' for help. Type '\c' to clear the current input statement.

mysql>
```

* webapp/sql/initdb.dディレクトリ配下のSQLが起動時に投入されるようになっています
* webapp/sql/init.sh を実行すると、webapp/sql/init.sqlが投入されてDBの全テーブルに対してDELETEクエリが発行されます (お掃除)

### webapp起動 (Go)

コンパイルすると、/tmp/isupipeが作成されます。

バイナリはLDFLAGSによってstrippedな状態でコンパイルされます。

```
isucon/isucon13/webapp/go$ make
go build -o /tmp/isupipe -ldflags "-s -w"
isucon/isucon13/webapp/go$ file /tmp/isupipe
/tmp/isupipe: ELF 64-bit LSB executable, x86-64, version 1 (SYSV), dynamically linked, interpreter /lib64/ld-linux-x86-64.so.2, Go BuildID=oPmA-hr6Ug6TbwITkQ1X/cPHWGWkU_PQcsampCrOX/DzLxtHPGuucLf6w_R665/RgfqXH-k0OrQOIB7E7jT, stripped
```

コンパイルできたバイナリを実行すると、Echoサーバが立ち上がります。
一旦、他と衝突しにくそうな 12345/tcpでリッスンするようにしています。

```
/isucon/isucon13/webapp/go$ make && /tmp/isupipe
go build -o /tmp/isupipe -ldflags "-s -w"

   ____    __
  / __/___/ /  ___
 / _// __/ _ \/ _ \
/___/\__/_//_/\___/ v4.11.1
High performance, minimalist Go web framework
https://echo.labstack.com
____________________________________O/_______
                                    O\
⇨ http server started on [::]:12345
```

sqlxパッケージを用いた基本的なCRUDはlivestream_handler.goに書きましたので、ご参考まで。