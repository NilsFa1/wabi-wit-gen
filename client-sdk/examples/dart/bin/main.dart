// Plain application code against the generated Dart SDK — the same code runs
// in a Flutter app. Which core runs underneath (Rust, Go, Zig) is decided by
// the native library that gets loaded, not here.
import 'dart:convert';
import 'dart:typed_data';

import 'package:wabi_client/wabi_client.dart';

Future<void> main(List<String> args) async {
  final client = WabiClient(ClientOptions(
    endpoint: args.isNotEmpty ? args.first : 'mem://example',
    apiKey: 'demo-key',
    requestTimeoutMs: 5000,
  ));

  final listener = () async {
    await for (final event in client.events()) {
      print('  event: $event');
      if (event is ClientEventMessageReceived) break; // releases the native stream
    }
  }();

  await client.connect();
  final customer = await client.getCustomer('4711');
  print('customer: ${customer.name} <${customer.email ?? '-'}>');

  try {
    await client.getCustomer('0000');
  } on ClientErrorNotFound {
    print('customer 0000: not found');
  }

  print('created: ${await client.createCustomer('Max Mustermann', null)}');
  await client.sendCommand(Command(name: 'broadcast', payload: Uint8List.fromList(utf8.encode('hello from Dart'))));

  await listener;
  await client.disconnect();
  print('state: ${client.state()}');
  client.close();
}
