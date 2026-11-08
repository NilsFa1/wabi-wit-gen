// A Flutter app on the example SDK. The same code runs on Android, iOS,
// macOS, Windows and Linux; the core library for each platform comes from the
// SDK's build hook (Dart native assets).
import 'dart:async';

import 'package:demo_client/demo_client.dart';
import 'package:flutter/material.dart';

import 'app_keys.dart';

void main() => runApp(const DemoExampleApp());

class DemoExampleApp extends StatelessWidget {
  const DemoExampleApp({super.key, this.endpoint = 'mem://flutter-example'});

  final String endpoint;

  @override
  Widget build(BuildContext context) => MaterialApp(
        title: 'Wabi client',
        home: Scaffold(
          appBar: AppBar(title: const Text('Wabi client')),
          body: Padding(padding: const EdgeInsets.all(16), child: CustomerView(endpoint: endpoint)),
        ),
      );
}

class CustomerView extends StatefulWidget {
  const CustomerView({super.key, required this.endpoint});

  final String endpoint;

  @override
  State<CustomerView> createState() => _CustomerViewState();
}

class _CustomerViewState extends State<CustomerView> {
  final _cancel = CancellationToken();
  late final DemoClient _client;
  StreamSubscription<ClientEvent>? _events;
  String _core = '';
  String _status = 'connecting …';
  final _log = <String>[];

  @override
  void initState() {
    super.initState();
    _core = NativeBackend.shared.description;
    _client = DemoClient(
        ClientOptions(endpoint: widget.endpoint, apiKey: 'demo-key', requestTimeoutMs: 10000, keyId: 'app-key'));
    // The app keeps the signing key; the core asks for every signature (host
    // calls). The registration belongs to this client and ends with it.
    ClientHost.registerFor(_client, AppKeys());
    _events = _client.events().listen((e) => setState(() => _log.add('event: $e')));
    _run();
  }

  Future<void> _run() async {
    try {
      await _client.connect();
      final key = await _client.keyFingerprint(); // the key the backend verified
      if (!mounted) return;
      setState(() => _log.add('signed with key ${key.substring(0, 16)}…'));
      final customer = await _client.getCustomer('4711', cancellationToken: _cancel);
      if (!mounted) return;
      setState(() => _status = 'customer: ${customer.name}');
      final created = await _client.createCustomer('Max Mustermann', null);
      if (!mounted) return;
      setState(() => _log.add('created: ${created.name} (${created.id})'));
    } on WabiCancelledException {
      // widget disposed
    } catch (e) {
      if (mounted) setState(() => _status = 'error: $e');
    }
  }

  @override
  void dispose() {
    _cancel.cancel(); // cancels the running request inside the core
    _events?.cancel(); // releases the native stream
    _client.close();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(_core, key: const Key('core'), style: Theme.of(context).textTheme.bodySmall),
          const SizedBox(height: 12),
          Text(_status, key: const Key('status'), style: Theme.of(context).textTheme.titleMedium),
          const SizedBox(height: 12),
          for (final line in _log) Text(line),
        ],
      );
}
